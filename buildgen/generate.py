#!/usr/bin/env python3
"""
VeldMap Code Generator

Reads schema.yaml + config.yaml for a module and generates:
  - generated/src/lib.rs       (WASM entry points + dispatch table)
  - generated/Cargo.toml       (standalone workspace, all deps)
  - generated/build.rs         (prost codegen)
  - generated/rust-toolchain.toml
  - generated/.cargo/config.toml

Before rendering, the schema is validated: every `type:` must name an existing
proto message, every sub/call must exist on the dependency's schema, and the
declared types must match on both sides. The schema is the single source of
truth — validation failures abort generation with the full error list, and the
generated emit/call stubs are typed with the exact message from the schema.
"""
import os
import re
import shutil
import argparse
import yaml
from jinja2 import Environment, FileSystemLoader


# ── Helpers ───────────────────────────────────────────────────────────────────

def yaml_dep_to_toml(val) -> str:
    """Convert a YAML dependency value to a TOML inline-table or version string.

    Examples:
        "0.14"                              → '"0.14"'
        {version: "1.0", features: [...]}  → '{ version = "1.0", features = [...] }'
        {path: "../../foo/generated"}       → '{ path = "../../foo/generated" }'
    """
    if isinstance(val, str):
        return f'"{val}"'
    if isinstance(val, dict):
        parts = []
        for k, v in val.items():
            if isinstance(v, str):
                parts.append(f'{k} = "{v}"')
            elif isinstance(v, bool):
                parts.append(f'{k} = {"true" if v else "false"}')
            elif isinstance(v, list):
                items = ", ".join(f'"{x}"' for x in v)
                parts.append(f'{k} = [{items}]')
            else:
                parts.append(f'{k} = {v}')
        return "{ " + ", ".join(parts) + " }"
    return str(val)


def collect_proto_info(proto_file: str) -> tuple[str | None, set[str]]:
    """Read the package declaration and all message names from a .proto file."""
    package = None
    messages = set()
    with open(proto_file) as f:
        for line in f:
            stripped = line.strip()
            if stripped.startswith("package "):
                package = stripped.split()[1].rstrip(";")
            m = re.match(r"message\s+(\w+)", stripped)
            if m:
                messages.add(m.group(1))
    return package, messages


def read_proto_package(proto_file: str) -> str | None:
    """Read the 'package' declaration from a .proto file."""
    return collect_proto_info(proto_file)[0]


def rust_type_name(name: str) -> str:
    """Convert a proto message name to its prost-generated Rust name.

    prost-build applies heck::ToUpperCamelCase: UIEvent → UiEvent.
    Schema `type:` fields hold proto names, so the conversion must match.
    """
    words = re.findall(r'[A-Z]+(?![a-z])|[A-Z][a-z0-9]*|[a-z0-9]+', name)
    return "".join(w[0].upper() + w[1:].lower() for w in words)


def schema_type_to_rust_path(type_str: str) -> str:
    """`app/UIEvent` → `app::UiEvent` (module path + prost type name)."""
    mod, _, name = type_str.partition("/")
    return f"{mod}::{rust_type_name(name)}" if name else ""


# ── Schema validation ─────────────────────────────────────────────────────────
#
# The type universe maps a proto package alias (last segment of the proto
# `package`) to its message names and its origin: "core" for veldcore/interface
# (platform contracts at its root plus native services under interface/modules/),
# otherwise the directory name of the wasm module that owns types.proto.

def iter_core_files(iface_dir: str, suffix: str):
    """Single walk over veldcore/interface: root-level `*<suffix>` files
    (platform contracts: core, graphics, ...), then one file per native
    service at modules/<name>/<name><suffix>."""
    for f in sorted(os.listdir(iface_dir)):
        path = os.path.join(iface_dir, f)
        if f.endswith(suffix) and os.path.isfile(path):
            yield path
    modules_dir = os.path.join(iface_dir, "modules")
    if os.path.isdir(modules_dir):
        for name in sorted(os.listdir(modules_dir)):
            path = os.path.join(modules_dir, name, f"{name}{suffix}")
            if os.path.exists(path):
                yield path


def build_type_universe(proto_dir: str, modules_root: str | None) -> dict:
    universe = {}

    def add(alias, messages, origin, source):
        if alias in universe:
            raise SystemExit(
                f"Proto package alias collision: '{alias}' is defined by both "
                f"{universe[alias]['source']} and {source}")
        universe[alias] = {"messages": messages, "origin": origin, "source": source}

    for path in iter_core_files(proto_dir, ".proto"):
        pkg, messages = collect_proto_info(path)
        if pkg:
            add(pkg.split(".")[-1], messages, "core", path)

    if modules_root and os.path.isdir(modules_root):
        for name in sorted(os.listdir(modules_root)):
            tp = os.path.join(modules_root, name, "types.proto")
            if os.path.exists(tp):
                pkg, messages = collect_proto_info(tp)
                if pkg:
                    add(pkg.split(".")[-1], messages, name, tp)

    return universe


# Корреляция запрос/ответ едет в конверте (EventEnvelope.correlation_id), а не
# полем доменного сообщения: заказчик отличает свой ответ от чужого, не тратя
# на это место в контракте типа. `replies_to` на выходе объявляет пару — по
# ней же генерируются стабы, требующие correlation_id аргументом.
def correlated_topics(schema: dict) -> tuple[dict[str, list[str]], set[str]]:
    """Топики сервиса, участвующие в паре `replies_to`.

    Возвращает (запрос → его ответы, множество ответов). Ответов у запроса
    может быть несколько: у `network/on_fs_download` их два — прогресс и
    результат, и корреляцию несут оба.

    Всё остальное корреляции не несёт: у события «состояние изменилось»
    корреспондента нет, и стаб для него её не принимает — забыть или
    приписать её лишнему топику нельзя по построению.
    """
    outputs = (schema.get("interface") or {}).get("outputs") or {}
    pairs = [(n, (e or {}).get("replies_to")) for n, e in outputs.items()]
    replies = {n for n, r in pairs if r}
    requests: dict[str, list[str]] = {}
    for reply, request in pairs:
        if request:
            requests.setdefault(request, []).append(reply)
    return requests, replies


# ── Поток операции: чем она кончается и можно ли её убить ────────────────────
# Событие в полёте — это и есть задача, поэтому отдельных «завести/закрыть» у
# платформы нет: хост открывает учёт, когда публикуется запрос, объявленный
# `cancellable: true`, и закрывает, когда проходит его терминальный ответ.
# Какой из ответов терминальный, знает только схема: у `network/on_fs_download`
# их два (прогресс и результат), и лишь второй означает конец работы.

def terminal_reply_of(schema: dict) -> tuple[dict[str, str], list[str]]:
    """Запрос → имя его терминального ответа, плюс список ошибок.

    Единственный ответ терминален по умолчанию — писать `terminal: true` там,
    где выбора нет, значило бы заставлять повторять очевидное. Ключ обязателен
    ровно тогда, когда ответов несколько и решение действительно есть.
    """
    outputs = (schema.get("interface") or {}).get("outputs") or {}
    requests, _ = correlated_topics(schema)
    terminal, errors = {}, []
    for request, replies in requests.items():
        marked = [r for r in replies if (outputs.get(r) or {}).get("terminal")]
        if len(replies) == 1 and not marked:
            terminal[request] = replies[0]
        elif len(marked) == 1:
            terminal[request] = marked[0]
        elif not marked:
            errors.append(f"interface.inputs.{request}: у запроса несколько ответов "
                          f"({', '.join(sorted(replies))}) — ровно один из них должен "
                          f"нести `terminal: true`")
        else:
            errors.append(f"interface.inputs.{request}: `terminal: true` стоит у "
                          f"нескольких ответов ({', '.join(sorted(marked))}) — "
                          f"терминальный ответ у запроса один")
    return terminal, errors


def flow_entries(svc_name: str, schema: dict) -> tuple[list[dict], list[str]]:
    """Отменяемые запросы сервиса в виде записей для таблицы потока хоста.

    Отменяемость объявляет исполнитель у себя во входе: только он знает, что
    работа бывает достаточно долгой, чтобы её имело смысл убивать. Заказчику
    объявлять нечего — он лишь публикует запрос.
    """
    inputs = (schema.get("interface") or {}).get("inputs") or {}
    requests, _ = correlated_topics(schema)
    terminal, errors = terminal_reply_of(schema)
    entries = []
    for name, entry in inputs.items():
        if not (entry or {}).get("cancellable"):
            continue
        if name not in terminal:
            # Про неоднозначный терминальный ответ уже сказано выше — второй
            # раз о том же, да ещё и другими словами, только путает.
            if name not in requests:
                errors.append(f"interface.inputs.{name}: `cancellable: true` требует ответа "
                              f"(`replies_to`) — иначе учёт операции некому закрыть")
            continue
        entries.append({
            "request":  f"{svc_name}/{name}",
            "terminal": f"{svc_name}/{terminal[name]}",
        })
    return entries, errors


# ── Нормализация: схема → модель сервиса ─────────────────────────────────────
# Общий для обоих конвейеров (wasm-модули и нативные сервисы хоста) шаг:
# флаги топиков вычисляются из схемы один раз, а тонкие бэкенды рендеринга
# различаются только резолвером rust_path и набором шаблонов.

def topic_entries(svc_name: str, schema: dict, kind: str, rust_path_of,
                  only: set | None = None) -> list[dict]:
    """Топики одного направления (`inputs`/`outputs`) с вычисленными флагами:
    `correlated`/`pairs_with` — из пар `replies_to`, `targeted` — из
    объявления топика. `rust_path_of(kind, name, entry)` — бэкенд-специфичный
    резолвер типа (хост и wasm раскладывают один тип по разным crate-путям).
    `only` ограничивает набор имён (потребителю нужны лишь объявленные им
    вызовы, а не все входы производителя)."""
    requests, replies = correlated_topics(schema)
    entries = []
    for n, d in ((schema.get("interface") or {}).get(kind) or {}).items():
        if only is not None and n not in only:
            continue
        d = d or {}
        # Корреспондент топика (в док-комментарий стаба): у входа-запроса —
        # его ответы, у выхода-ответа — его запрос.
        if kind == "inputs":
            peers = requests.get(n, [])
            correlated = n in requests
        else:
            peers = [d.get("replies_to")]
            correlated = n in replies
        entries.append({
            "name":       n,
            "const":      n.upper(),
            "rust_path":  rust_path_of(kind, n, d),
            "targeted":   bool(d.get("targeted", False)),
            "correlated": correlated,
            "pairs_with": ", ".join(f"`{svc_name}/{t}`" for t in peers),
        })
    return entries


def service_model(svc_name: str, schema: dict, rust_path_of) -> dict:
    """Схема → нормализованная модель сервиса: inputs/outputs с флагами."""
    return {
        "name":    svc_name,
        "snake":   svc_name.replace("-", "_"),
        "inputs":  topic_entries(svc_name, schema, "inputs", rust_path_of),
        "outputs": topic_entries(svc_name, schema, "outputs", rust_path_of),
    }


def load_dep_schema(dep: str, modules_root: str, proto_dir: str) -> dict | None:
    """Схема зависимости: соседний wasm-модуль либо платформенный нативный
    сервис (interface/modules/<dep>/<dep>.schema.yaml)."""
    for p in (os.path.join(modules_root, dep, "schema.yaml"),
              os.path.join(proto_dir, "modules", dep, f"{dep}.schema.yaml")):
        if os.path.exists(p):
            with open(p) as f:
                return yaml.safe_load(f) or {}
    return None


def local_package_alias(module_dir: str) -> str | None:
    """Alias of the module's own types.proto package, if any."""
    tp = os.path.join(module_dir, "types.proto")
    if os.path.exists(tp):
        pkg = read_proto_package(tp)
        if pkg:
            return pkg.split(".")[-1]
    return None


def validate_service_schema(schema: dict, universe: dict,
                            allow_empty_payload: bool,
                            schema_dir: str | None = None,
                            proto_dir: str | None = None) -> tuple[list[str], dict]:
    """Validate a service schema against the type universe and the schemas of
    its dependencies. Returns (errors, resolved) where resolved maps every
    type reference to its canonical 'alias/Message' form:
        resolved["inputs"][name], resolved["outputs"][name],
        resolved["subs"][(dep, sub)], resolved["calls"][(dep, call)]

    Оба диалекта схем — один валидатор; различаются они одним параметром:
    платформенный сервис МОЖЕТ иметь топик без payload (app/on_ready) —
    `allow_empty_payload=True`, wasm-модуль — нет (обязателен `type`).
    Перекрёстная проверка dependencies есть только у wasm-модуля (задан
    schema_dir): платформенные сервисы зависимостей не объявляют.

    A topic's payload type is declared exactly once, in the interface of the
    module that owns it (interface.outputs / interface.inputs). dependencies.
    *.subs and .calls just list the topic names consumed; their types are
    derived here from the producer's own schema — never redeclared.
    """
    errors = []
    name = schema.get("name")
    iface = schema.get("interface") or {}
    deps = schema.get("dependencies") or {}
    resolved = {"inputs": {}, "outputs": {}, "subs": {}, "calls": {}}

    own_alias = local_package_alias(schema_dir) if schema_dir else None
    # Чужие (не core) пакеты разрешены только через объявленные зависимости —
    # диалект wasm-модуля; у платформенного сервиса зависимостей не бывает,
    # поэтому для него остаётся чистое правило «только core».
    dep_aliases = ({a for a, info in universe.items() if info["origin"] in deps}
                   if schema_dir else set())

    def err(where, msg):
        errors.append(f"{where}: {msg}")

    def check(where, type_str) -> str | None:
        """Validate one type reference; return canonical 'alias/Message'."""
        if not type_str:
            if not allow_empty_payload:
                err(where, "missing 'type'")
            return None
        alias, _, tname = type_str.partition("/")
        if not tname:
            err(where, f"malformed type '{type_str}' (expected '<package>/<Message>')")
            return None
        if alias == "module":
            if not own_alias:
                err(where, f"type '{type_str}' uses 'module/' but this module has no types.proto")
                return None
            alias = own_alias
        info = universe.get(alias)
        if info is None:
            err(where, f"unknown proto package '{alias}' in '{type_str}' "
                       f"(known: {', '.join(sorted(universe))})")
            return None
        if tname not in info["messages"]:
            err(where, f"message '{tname}' not found in package '{alias}' "
                       f"(has: {', '.join(sorted(info['messages']))})")
            return None
        if alias != own_alias and info["origin"] != "core" and alias not in dep_aliases:
            err(where, f"type '{type_str}' belongs to module '{info['origin']}', "
                       f"which is not declared in dependencies")
            return None
        return f"{alias}/{tname}"

    for input_name, entry in (iface.get("inputs") or {}).items():
        c = check(f"interface.inputs.{input_name}", (entry or {}).get("type"))
        if c:
            resolved["inputs"][input_name] = c

    for output_name, entry in (iface.get("outputs") or {}).items():
        c = check(f"interface.outputs.{output_name}", (entry or {}).get("type"))
        if c:
            resolved["outputs"][output_name] = c

    for output_name, entry in (iface.get("outputs") or {}).items():
        replies_to = (entry or {}).get("replies_to")
        if not replies_to:
            continue
        where = f"interface.outputs.{output_name}.replies_to"
        if replies_to not in (iface.get("inputs") or {}):
            err(where, f"'{replies_to}' is not one of this service's interface.inputs")

    # Терминальный ответ и отменяемость: учёт операции ведёт хост, и открыть
    # его без ответа, которым он закрывается, нельзя (см. flow_entries).
    errors.extend(flow_entries(name, schema)[1])

    # Перекрёстная проверка зависимостей — только диалект wasm-модуля.
    if schema_dir is not None:
        modules_root = os.path.dirname(schema_dir)
        for dep, dep_data in deps.items():
            dep_data = dep_data or {}
            dep_schema = load_dep_schema(dep, modules_root, proto_dir)
            if dep_schema is None:
                err(f"dependencies.{dep}", "no schema found (neither a sibling module "
                                           "nor a veldcore/interface/modules service)")
                continue

            # In the dependency's own schema 'module/' refers to *its* package.
            dep_alias = local_package_alias(os.path.join(modules_root, dep)) or dep
            dep_iface = dep_schema.get("interface") or {}
            dep_outputs = dep_iface.get("outputs") or {}
            dep_inputs = dep_iface.get("inputs") or {}

            def cross_check(kind, declared_topics, contract, contract_kind):
                for topic in declared_topics:
                    where = f"dependencies.{dep}.{kind}.{topic}"
                    if topic not in contract:
                        err(where, f"'{dep}' declares no {contract_kind} '{topic}' "
                                   f"({contract_kind}s: {', '.join(sorted(contract)) or '—'})")
                        continue
                    theirs_raw = (contract[topic] or {}).get("type") or ""
                    if not theirs_raw:
                        err(where, f"'{dep}' {contract_kind} '{topic}' has no payload type "
                                   f"and cannot be used as a typed dependency {kind[:-1]}")
                        continue
                    alias, _, tname = theirs_raw.partition("/")
                    theirs = f"{dep_alias}/{tname}" if alias == "module" else theirs_raw
                    c = check(where, theirs)
                    if c:
                        resolved[kind][(dep, topic)] = c

            cross_check("subs", list(dep_data.get("subs") or []), dep_outputs, "output")
            cross_check("calls", list(dep_data.get("calls") or []), dep_inputs, "input")

    return [f"schema '{name}': {e}" for e in errors], resolved


def validate_module_schema(schema: dict, schema_dir: str, proto_dir: str,
                           universe: dict) -> tuple[list[str], dict]:
    """Wasm-модуль: payload обязателен, dependencies проверяются перекрёстно."""
    return validate_service_schema(schema, universe, allow_empty_payload=False,
                                   schema_dir=schema_dir, proto_dir=proto_dir)


def validate_core_schema(svc_schema: dict, universe: dict) -> list[str]:
    """Платформенный сервис veldcore: топик может быть без payload (`{}`,
    напр. app/on_ready)."""
    errors, _ = validate_service_schema(svc_schema, universe, allow_empty_payload=True)
    return errors


def fail_on(errors: list[str], schema_path: str):
    if errors:
        print(f"❌ Schema validation failed for {schema_path}:")
        for e in errors:
            print(f"   - {e}")
        raise SystemExit(1)


def validate_schema_identity(schema: dict, schema_path: str) -> list[str]:
    """`name:` обязан совпадать с тем, как модуль зовётся снаружи.

    Производитель строит свои топики из `schema.name`, а потребитель — из
    ключа в `dependencies:`, то есть из имени каталога (см. generate_module:
    `f"{name}/{input}"` против `f"{dep_name}/{sub}"`). Хост, в свою очередь,
    ищет конфиг как `<config_dir>/<name>.json`. Разъехавшись, эти три строки
    не дают ни ошибки компиляции, ни ошибки загрузки: событие просто никогда
    не доставляется. Поэтому равенство проверяется здесь, до генерации.

    Признак вида схемы — имя файла: `schema.yaml` у wasm-модуля (имя задаёт
    каталог), `<X>.schema.yaml` у сервиса платформы (имя задаёт сам файл).
    """
    name = schema.get("name")
    if not name:
        return [f"schema has no 'name'"]

    basename = os.path.basename(schema_path)
    if basename == "schema.yaml":
        expected = os.path.basename(os.path.dirname(schema_path))
        source = "имя каталога модуля"
    elif basename.endswith(".schema.yaml"):
        expected = basename[: -len(".schema.yaml")]
        source = "имя файла схемы"
    else:
        return []

    if name != expected:
        return [
            f"name: '{name}' не совпадает с '{expected}' ({source}). "
            f"Потребители адресуют этот сервис как '{expected}/<топик>', "
            f"а он публикует в '{name}/<топик>' — события не дойдут. "
            f"Переименуйте и то, и другое."
        ]
    return []


# ── Host bindings generation ─────────────────────────────────────────────────

def generate_host_bindings(args, script_dir: str):
    """Generate the host bindings crate (proto types + topic stubs) from
    veldcore/interface: root-level platform contracts (core.proto, app.proto, ...)
    plus native services under interface/modules/<name>/. Used by the host
    implementation; the crate itself depends only on prost (host implements
    Publisher)."""
    out_dir   = os.path.abspath(args.host_bindings)
    proto_dir = os.path.abspath(args.proto_dir)
    package   = args.package or "veldmap-host-bindings"

    universe = build_type_universe(proto_dir, None)

    proto_files = [os.path.relpath(p, proto_dir).replace("\\", "/")
                   for p in iter_core_files(proto_dir, ".proto")]
    proto_packages = []
    for f in proto_files:
        pkg_decl = read_proto_package(os.path.join(proto_dir, f))
        proto_packages.append({"file": pkg_decl, "mod": pkg_decl.split(".")[-1]})

    schema_files = list(iter_core_files(proto_dir, ".schema.yaml"))

    services = []
    flow = []
    for schema_path in schema_files:
        with open(schema_path) as sf:
            svc_schema = yaml.safe_load(sf)
        fail_on(validate_schema_identity(svc_schema, schema_path)
                + validate_core_schema(svc_schema, universe), schema_path)
        services.append(service_model(
            svc_schema.get("name"), svc_schema,
            lambda _kind, _n, d: schema_type_to_rust_path(d.get("type") or "")))
        flow.extend(flow_entries(svc_schema.get("name"), svc_schema)[0])

    # ── Таблица потока: по всему дереву, а не только по veldcore/interface ───
    # Учёт операций ведёт диспетчер, и топики wasm-модулей идут через него
    # наравне с платформенными: отменяемый декод живёт в image-loader. Схемы
    # модулей к этому моменту уже проверены (build.py генерирует их раньше),
    # поэтому здесь они только читаются.
    project_root = os.path.normpath(os.path.join(script_dir, ".."))
    modules_root = os.path.join(project_root, "veldmodules")
    for entry in sorted(os.scandir(modules_root), key=lambda e: e.name):
        module_schema_path = os.path.join(entry.path, "schema.yaml")
        if not entry.is_dir() or not os.path.exists(module_schema_path):
            continue
        with open(module_schema_path) as sf:
            module_schema = yaml.safe_load(sf)
        entries, errors = flow_entries(module_schema.get("name"), module_schema)
        fail_on(errors, module_schema_path)
        flow.extend(entries)

    proto_dir_rel = os.path.relpath(proto_dir, out_dir).replace("\\", "/")
    template_data = {
        "package":        package,
        "proto_files":    proto_files,
        "proto_packages": proto_packages,
        "proto_dir_rel":  proto_dir_rel,
        "services":       services,
        # Таблицы ищутся бинарным поиском — обе отсортированы по ключу.
        "cancellable":    sorted(flow, key=lambda e: e["request"]),
        "terminal":       sorted(e["terminal"] for e in flow),
    }

    env = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))
    renders = {
        os.path.join(out_dir, "src", "lib.rs"): env.get_template("host_lib.rs.j2"),
        os.path.join(out_dir, "Cargo.toml"):    env.get_template("host_Cargo.toml.j2"),
        os.path.join(out_dir, "build.rs"):      env.get_template("host_build.rs.j2"),
    }
    for path, template in renders.items():
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(template.render(template_data))

    print(f"✅ Generated host bindings at {out_dir}")

    # ── Крейты нативных модулей хоста (modules/<svc>/generated) ─────────────
    # Сервис с входами считается нативным модулем этой реализации хоста,
    # если рядом с generated/ существует каталог modules/<svc> с config.yaml.
    # Как и у wasm-модулей: generated — крейт, src/module.rs — гость через #[path].
    host_dir = os.path.dirname(out_dir)
    modules_dir = os.path.join(host_dir, "modules")
    # Имя сервиса → имя крейта его нативной реализации: нужно раннерам,
    # которые собирают модули по именам сервисов из runner.yaml.
    module_crates = {}
    for svc in services:
        if not svc["inputs"]:
            continue
        module_dir = os.path.join(modules_dir, svc["name"])
        config_path = os.path.join(module_dir, "config.yaml")
        if not os.path.isdir(module_dir) or not os.path.exists(config_path):
            continue

        with open(config_path) as cf:
            module_config = yaml.safe_load(cf) or {}
        raw_deps = (module_config.get("rust", {}) or {}).get("dependencies", {}) or {}
        module_data = dict(svc)
        module_data["package"] = module_config.get("package", svc["name"])
        module_data["dependencies"] = {
            name: yaml_dep_to_toml(val) for name, val in raw_deps.items()
        }
        module_crates[svc["name"]] = module_data["package"]

        gen_dir = os.path.join(module_dir, "generated")
        module_renders = {
            os.path.join(gen_dir, "src", "lib.rs"): env.get_template("host_module_lib.rs.j2"),
            os.path.join(gen_dir, "Cargo.toml"):    env.get_template("host_module_Cargo.toml.j2"),
        }
        for path, template in module_renders.items():
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write(template.render(module_data))
        print(f"✅ Generated host module crate at {gen_dir}")

    generate_runner_crates(host_dir, module_crates, env)


def generate_runner_crates(host_dir: str, module_crates: dict, env) -> None:
    """Generate the composition-root crate for every runner that declares one.

    A runner is a directory under runners/ holding a runner.yaml that lists the
    native modules it composes. The набор of modules is a property of the runner
    (desktop and mobile builds take different lists), so it lives in data rather
    than in #[cfg(target_os)] branches inside the runner's own source.
    """
    runners_dir = os.path.join(host_dir, "runners")
    if not os.path.isdir(runners_dir):
        return

    for runner in sorted(os.listdir(runners_dir)):
        runner_dir = os.path.join(runners_dir, runner)
        runner_yaml = os.path.join(runner_dir, "runner.yaml")
        if not os.path.exists(runner_yaml):
            continue

        with open(runner_yaml) as rf:
            runner_config = yaml.safe_load(rf) or {}

        package = runner_config.get("package")
        if not package:
            raise SystemExit(f"{runner_yaml}: 'package' is required")

        modules = []
        for name in runner_config.get("modules", []) or []:
            crate = module_crates.get(name)
            if crate is None:
                known = ", ".join(sorted(module_crates)) or "<none>"
                raise SystemExit(
                    f"{runner_yaml}: module '{name}' has no native implementation "
                    f"(expected {os.path.join('modules', name, 'config.yaml')} plus a "
                    f"service schema with inputs). Available: {known}")
            modules.append({
                "name": name,
                "crate": crate,
                "crate_snake": crate.replace("-", "_"),
            })

        gen_dir = os.path.join(runner_dir, "generated")
        renders = {
            os.path.join(gen_dir, "src", "lib.rs"): env.get_template("runner_lib.rs.j2"),
            os.path.join(gen_dir, "Cargo.toml"):    env.get_template("runner_Cargo.toml.j2"),
        }
        data = {"runner": runner, "package": package, "modules": modules}
        for path, template in renders.items():
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write(template.render(data))
        print(f"✅ Generated runner composition crate at {gen_dir}")


# ── Main ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Generate Rust bindings from schema.yaml")
    parser.add_argument("--schema",        help="Absolute path to schema.yaml")
    parser.add_argument("--output-dir",    help="Absolute path to output directory")
    parser.add_argument("--host-bindings", help="Generate host bindings crate to this dir and exit")
    parser.add_argument("--proto-dir",     help="Dir with *.proto and *.schema.yaml (host-bindings mode)")
    parser.add_argument("--package",       help="Package name (host-bindings mode)")
    args = parser.parse_args()

    script_dir   = os.path.dirname(os.path.abspath(__file__))
    project_root = os.path.normpath(os.path.join(script_dir, ".."))
    core_proto_dir = os.path.join(project_root, "veldcore", "interface")

    # ── Host bindings mode (--host-bindings) ─────────────────────────────────
    if args.host_bindings:
        if not args.proto_dir:
            parser.error("--proto-dir is required with --host-bindings")
        generate_host_bindings(args, script_dir)
        return

    if not args.schema:
        parser.error("--schema is required unless --host-bindings is given")

    schema_path = os.path.abspath(args.schema)

    # ── Load schema ──────────────────────────────────────────────────────────
    with open(schema_path) as f:
        schema = yaml.safe_load(f)

    if not args.output_dir:
        parser.error("--output-dir is required")

    output_dir  = os.path.abspath(args.output_dir)
    schema_dir  = os.path.dirname(schema_path)

    config_data = {}
    config_path = os.path.join(schema_dir, "config.yaml")
    if os.path.exists(config_path):
        with open(config_path) as f:
            config_data = yaml.safe_load(f)

    name         = schema.get("name")
    package_name = config_data.get("package", name)
    version      = schema.get("version", "0.1.0")
    rust_config  = config_data.get("rust", {})

    # ── Validate the schema before rendering anything ────────────────────────
    universe = build_type_universe(core_proto_dir, os.path.dirname(schema_dir))
    errors, resolved = validate_module_schema(schema, schema_dir, core_proto_dir, universe)
    fail_on(validate_schema_identity(schema, schema_path) + errors, schema_path)

    def module_rust_path(canonical: str) -> str:
        """Rust path of a canonical 'alias/Message' inside the module crate."""
        alias, tname = canonical.split("/")
        rust_name = rust_type_name(tname)
        if universe[alias]["origin"] == "core":
            return f"veldsdk::proto::{alias}::{rust_name}"
        return f"crate::proto::{alias}::{rust_name}"

    # ── Нормализованная модель сервиса (общий шаг обоих конвейеров) ──────────
    # Корреляция едет в конверте (EventEnvelope.correlation_id), поэтому стабы
    # топиков из пар `replies_to` принимают её отдельным аргументом, а у всех
    # прочих её негде и указать. Свои пары — из собственной схемы, чужие — из
    # схемы зависимости: какой её выход отвечает какому входу, знает только она.
    own_model = service_model(
        name, schema,
        lambda kind, n, _d: module_rust_path(resolved[kind][n]))

    modules_root = os.path.dirname(schema_dir)
    dep_schemas = {}
    for dep_name in schema.get("dependencies", {}):
        dep_schemas[dep_name] = load_dep_schema(dep_name, modules_root, core_proto_dir) or {}

    # ── Build handler dispatch table (topic → handler + payload type) ────────
    handlers = []

    # Handler name = schema key, verbatim (no injected on_input_/on_sub_
    # prefix): the schema is expected to already name it `on_*`, so the
    # topic key and the Rust function it maps to are the same string.
    for entry in own_model["inputs"]:
        handlers.append({
            "topic":     f"{name}/{entry['name']}",
            "handler":   f"crate::module::{entry['name']}",
            "rust_path": entry["rust_path"],
        })

    for dep_name, dep_data in schema.get("dependencies", {}).items():
        for sub_name in (dep_data or {}).get("subs", {}) or {}:
            handlers.append({
                "topic":     f"{dep_name}/{sub_name}",
                "handler":   f"crate::module::{sub_name}",
                "rust_path": module_rust_path(resolved["subs"][(dep_name, sub_name)]),
            })

    # ── Typed emit/call stubs (schema is the source of truth for topics) ─────
    # interface.outputs  → crate::emit::<name>(&ExactMessage)
    # dependencies.*.calls → crate::calls::<dep_snake>::<name>(&ExactMessage)
    emits = own_model["outputs"]

    dep_calls = []
    for dep_name, dep_data in schema.get("dependencies", {}).items():
        calls = list((dep_data or {}).get("calls", {}) or {})
        if calls:
            dep_inputs = {e["name"]: e for e in topic_entries(
                dep_name, dep_schemas[dep_name], "inputs",
                lambda _kind, n, _d: module_rust_path(resolved["calls"][(dep_name, n)]),
                only=set(calls))}
            # Отменяемость объявляет исполнитель у себя во входе — здесь она
            # только считывается: заказчик получает стаб убийства ровно на те
            # вызовы, которые исполнитель разрешил убивать.
            dep_cancellable = {e["request"].split("/", 1)[1]
                               for e in flow_entries(dep_name, dep_schemas[dep_name])[0]}
            dep_calls.append({
                "service": dep_name,
                "snake": dep_name.replace("-", "_"),
                "methods": [dep_inputs[c] for c in calls],
                "cancellable": [c for c in calls if c in dep_cancellable],
            })

    # ── Hooks (runtime lifecycle callbacks, not tied to a topic) ──────────────
    # `hooks:` in schema.yaml lists which lifecycle hooks the module opts into.
    # `hook_event` re-runs `crate::module::hook_event(state)` after every
    # handled message (and once on `app/ready`); what it does — render a view,
    # or anything else — is entirely up to that hand-written function. The
    # generator does not need to know or resolve which dependency it talks to.
    hooks = set(schema.get("hooks") or [])
    hook_event = "hook_event" in hooks

    # ── Detect local proto / wraps ───────────────────────────────────────────
    has_local_proto = os.path.exists(os.path.join(schema_dir, "types.proto"))
    has_wrap        = os.path.exists(os.path.join(schema_dir, "wraps", "rust", "src", "wrap.rs"))

    # Relative path from output_dir to project root (used in build.rs paths)
    workspace_root_rel = os.path.relpath(project_root, output_dir)

    include_dirs = [
        workspace_root_rel,
        os.path.join(workspace_root_rel, "veldcore", "interface"),
    ]

    # ── Workspace Config ─────────────────────────────────────────────────────
    workspace_path = os.path.join(project_root, "workspace.yaml")
    workspace_data = {}
    if os.path.exists(workspace_path):
        with open(workspace_path) as f:
            workspace_data = yaml.safe_load(f) or {}

    sdk_base = workspace_data.get("workspace", {}).get("sdk", "veldcore/sdk")
    sdk_path = os.path.join(workspace_root_rel, sdk_base, "rust").replace("\\", "/")

    wrap_sdk_path = os.path.join(os.path.relpath(project_root, os.path.join(output_dir, "wraps", "rust")), sdk_base, "rust").replace("\\", "/")

    # ── Discover dependent protos (from schema.yaml dependencies) ─────────────
    raw_deps    = rust_config.get("dependencies", {})
    dep_protos  = []
    cargo_dependencies = {}

    # 1. Add explicitly defined third-party deps
    for dep_name, dep_val in raw_deps.items():
        cargo_dependencies[dep_name] = yaml_dep_to_toml(dep_val)

    # 2. Add schema-inferred internal dependencies
    schema_deps = schema.get("dependencies", {})
    for dep_name in schema_deps.keys():
        dep_dir = os.path.normpath(os.path.join(schema_dir, "..", dep_name))
        if os.path.isdir(dep_dir):
            dep_config_path = os.path.join(dep_dir, "config.yaml")
            dep_pkg_name = dep_name
            if os.path.exists(dep_config_path):
                with open(dep_config_path) as df:
                    dep_cfg = yaml.safe_load(df) or {}
                    dep_pkg_name = dep_cfg.get("package", dep_name)

            api_crate_name = f"{dep_pkg_name}-wrap"
            api_crate_snake = api_crate_name.replace("-", "_")

            # Зависимость на wrap-крейт — только если он вообще порождается,
            # то есть у зависимости есть свой types.proto (см. рендер ниже).
            # Иначе Cargo.toml ссылался бы на несуществующий путь.
            proto_file = os.path.join(dep_dir, "types.proto")
            if os.path.exists(proto_file):
                pkg = read_proto_package(proto_file)
                if pkg:
                    cargo_dependencies[api_crate_name] = \
                        f'{{ path = "../../{dep_name}/generated/wraps/rust" }}'
                    dep_snake = pkg.split(".")[-1]
                    dep_protos.append({
                        "snake": dep_snake,
                        "api_crate": api_crate_snake,
                    })

    # Стабов входных топиков wrap-крейт не получает: публиковать в чужой вход
    # вправе только потребитель, объявивший связь в своём
    # `dependencies.<dep>.calls` (см. wrap_lib.rs.j2). Иначе граф связей из
    # schema.yaml был бы неполным.

    # ── Local proto metadata ─────────────────────────────────────────────────
    local_proto_package = None
    local_proto_path    = None
    if has_local_proto:
        lp = os.path.join(schema_dir, "types.proto")
        rel_to_ws        = os.path.relpath(lp, project_root)
        local_proto_path = os.path.join(workspace_root_rel, rel_to_ws)
        local_proto_package = read_proto_package(lp)

    # ── Template context ─────────────────────────────────────────────────────
    module_name_snake = package_name.replace("-", "_")

    template_data = {
        "module_name":        package_name,
        "module_name_snake":  module_name_snake,
        "service_name":       name,
        "version":            version,
        "sdk_path":           sdk_path,
        "dependencies":       cargo_dependencies,
        "rust": {
            "config": "crate::module::Config",
            "state":  "crate::module::State",
            "init":   "crate::module::hook_init",
        },
        "handlers":           handlers,
        "emits":              emits,
        "dep_calls":          dep_calls,
        "hook_event":         hook_event,
        "has_local_proto":    has_local_proto,
        "local_proto_package": local_proto_package,
        "local_proto_path":   local_proto_path,
        "include_dirs":       include_dirs,
        "dep_protos":         dep_protos,
    }

    # ── Render templates ─────────────────────────────────────────────────────
    env = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))

    renders = {
        os.path.join(output_dir, "src", "lib.rs"):               env.get_template("lib.rs.j2"),
        os.path.join(output_dir, "Cargo.toml"):                   env.get_template("Cargo.toml.j2"),
        os.path.join(output_dir, "build.rs"):                     env.get_template("build.rs.j2"),
        os.path.join(output_dir, "rust-toolchain.toml"):          env.get_template("rust-toolchain.toml.j2"),
        os.path.join(output_dir, ".cargo", "config.toml"):        env.get_template("cargo-config.toml.j2"),
    }

    # ── Render API Crate (Wrap) ──────────────────────────────────────────────
    # Wrap-крейт существует только у модуля с собственным types.proto: он для
    # того и нужен, чтобы отдать потребителям типы производителя.
    if not has_local_proto:
        # Каталог generated/ между запусками не чистится, поэтому wrap от
        # прежней сборки пережил бы удаление types.proto и остался бы живым
        # входом для cargo — источник говорил бы одно, диск другое.
        stale_wrap = os.path.join(output_dir, "wraps")
        if os.path.isdir(stale_wrap):
            shutil.rmtree(stale_wrap)
    else:
        wrap_dir = os.path.join(output_dir, "wraps", "rust")
        wrap_renders = {
            os.path.join(wrap_dir, "src", "lib.rs"): env.get_template("wrap_lib.rs.j2"),
            os.path.join(wrap_dir, "Cargo.toml"):    env.get_template("wrap_Cargo.toml.j2"),
            os.path.join(wrap_dir, "build.rs"):      env.get_template("wrap_build.rs.j2"),
        }
        renders.update(wrap_renders)

        template_data["api_crate_name"] = f"{package_name}-wrap"
        template_data["proto_package"] = local_proto_package
        template_data["has_custom_wrap"] = has_wrap
        template_data["wrap_sdk_path"] = wrap_sdk_path
        template_data["include_proto_dir"] = os.path.join(os.path.relpath(project_root, wrap_dir), "veldcore", "interface").replace("\\", "/")

    for path, template in renders.items():
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(template.render(template_data))

    print(f"✅ Generated module at {output_dir}")


if __name__ == "__main__":
    main()
