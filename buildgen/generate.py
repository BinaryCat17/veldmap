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


def collect_message_fields(proto_file: str) -> dict[str, set[str]]:
    """Map each message name to the set of field names declared directly in
    it. Regex-based like collect_proto_info; assumes flat (non-nested)
    messages, which holds for every .proto in this repo today."""
    fields: dict[str, set[str]] = {}
    current = None
    depth = 0
    field_re = re.compile(r"^(?:repeated|optional)?\s*[\w.]+\s+(\w+)\s*=\s*\d+\s*;")
    with open(proto_file) as f:
        for raw in f:
            line = raw.strip()
            if current is None:
                m = re.match(r"message\s+(\w+)", line)
                if m:
                    current = m.group(1)
                    fields.setdefault(current, set())
                    depth = line.count("{") - line.count("}")
                continue
            if depth == 1:
                fm = field_re.match(line)
                if fm:
                    fields[current].add(fm.group(1))
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                current = None
                depth = 0
    return fields


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

def iter_core_proto_files(proto_dir: str):
    """Yield every platform .proto file: root-level contracts (core, app,
    graphics, ...) plus one per native service in modules/<name>/<name>.proto."""
    for f in sorted(os.listdir(proto_dir)):
        path = os.path.join(proto_dir, f)
        if f.endswith(".proto") and os.path.isfile(path):
            yield path
    modules_dir = os.path.join(proto_dir, "modules")
    if os.path.isdir(modules_dir):
        for name in sorted(os.listdir(modules_dir)):
            path = os.path.join(modules_dir, name, f"{name}.proto")
            if os.path.exists(path):
                yield path


def build_type_universe(proto_dir: str, modules_root: str | None) -> dict:
    universe = {}

    def add(alias, messages, fields, origin, source):
        if alias in universe:
            raise SystemExit(
                f"Proto package alias collision: '{alias}' is defined by both "
                f"{universe[alias]['source']} and {source}")
        universe[alias] = {"messages": messages, "fields": fields, "origin": origin, "source": source}

    for path in iter_core_proto_files(proto_dir):
        pkg, messages = collect_proto_info(path)
        if pkg:
            add(pkg.split(".")[-1], messages, collect_message_fields(path), "core", path)

    if modules_root and os.path.isdir(modules_root):
        for name in sorted(os.listdir(modules_root)):
            tp = os.path.join(modules_root, name, "types.proto")
            if os.path.exists(tp):
                pkg, messages = collect_proto_info(tp)
                if pkg:
                    add(pkg.split(".")[-1], messages, collect_message_fields(tp), name, tp)

    return universe


# A reply's payload must echo the request's correlation_id (see Correlator in
# the SDK) so a caller can tell its own response apart from a broadcast one.
# `replies_to` on an output declares that pairing; this is what's checked.
CORRELATION_FIELD = "correlation_id"


def has_field(universe: dict, canonical: str, field: str) -> bool:
    alias, _, tname = canonical.partition("/")
    return field in universe.get(alias, {}).get("fields", {}).get(tname, set())


def local_package_alias(module_dir: str) -> str | None:
    """Alias of the module's own types.proto package, if any."""
    tp = os.path.join(module_dir, "types.proto")
    if os.path.exists(tp):
        pkg = read_proto_package(tp)
        if pkg:
            return pkg.split(".")[-1]
    return None


def validate_module_schema(schema: dict, schema_dir: str, proto_dir: str,
                           universe: dict) -> tuple[list[str], dict]:
    """Validate a module schema against the type universe and the schemas of
    its dependencies. Returns (errors, resolved) where resolved maps every
    type reference to its canonical 'alias/Message' form:
        resolved["inputs"][name], resolved["outputs"][name],
        resolved["subs"][(dep, sub)], resolved["calls"][(dep, call)]

    A topic's payload type is declared exactly once, in the interface of the
    module that owns it (interface.outputs / interface.inputs). dependencies.
    *.subs and .calls just list the topic names consumed; their types are
    derived here from the producer's own schema — never redeclared.
    """
    errors = []
    name = schema.get("name")
    modules_root = os.path.dirname(schema_dir)
    own_alias = local_package_alias(schema_dir)
    iface = schema.get("interface") or {}
    deps = schema.get("dependencies") or {}
    resolved = {"inputs": {}, "outputs": {}, "subs": {}, "calls": {}}

    dep_aliases = {a for a, info in universe.items() if info["origin"] in deps}

    def err(where, msg):
        errors.append(f"{where}: {msg}")

    def check(where, type_str) -> str | None:
        """Validate one type reference; return canonical 'alias/Message'."""
        if not type_str:
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
            err(where, f"'{replies_to}' is not one of this module's interface.inputs")
            continue
        out_type = resolved["outputs"].get(output_name)
        in_type = resolved["inputs"].get(replies_to)
        if out_type is None or in_type is None:
            continue  # already reported above as a type error
        if not has_field(universe, in_type, CORRELATION_FIELD):
            err(where, f"request '{replies_to}' ({in_type}) has no '{CORRELATION_FIELD}' field")
        if not has_field(universe, out_type, CORRELATION_FIELD):
            err(where, f"reply '{output_name}' ({out_type}) has no '{CORRELATION_FIELD}' field")

    def load_dep_schema(dep):
        """A dependency is a sibling wasm module or a platform service (either
        a root-level contract or a native service under interface/modules/)."""
        for p in (os.path.join(modules_root, dep, "schema.yaml"),
                  os.path.join(proto_dir, f"{dep}.schema.yaml"),
                  os.path.join(proto_dir, "modules", dep, f"{dep}.schema.yaml")):
            if os.path.exists(p):
                with open(p) as f:
                    return yaml.safe_load(f) or {}
        return None

    for dep, dep_data in deps.items():
        dep_data = dep_data or {}
        dep_schema = load_dep_schema(dep)
        if dep_schema is None:
            err(f"dependencies.{dep}", "no schema found (neither a sibling module "
                                       "nor a veldcore/interface *.schema.yaml service)")
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


def validate_core_schema(svc_schema: dict, universe: dict) -> list[str]:
    """Validate a veldcore service schema: every type must resolve to a
    veldcore proto message. Payload-less topics (`{}`) are allowed."""
    errors = []
    name = svc_schema.get("name")
    iface = svc_schema.get("interface") or {}
    for kind in ("inputs", "outputs"):
        for topic, entry in (iface.get(kind) or {}).items():
            type_str = (entry or {}).get("type")
            if not type_str:
                continue
            alias, _, tname = type_str.partition("/")
            info = universe.get(alias)
            if info is None or info["origin"] != "core":
                errors.append(f"schema '{name}': interface.{kind}.{topic}: unknown "
                              f"veldcore proto package '{alias}' in '{type_str}'")
            elif tname not in info["messages"]:
                errors.append(f"schema '{name}': interface.{kind}.{topic}: message "
                              f"'{tname}' not found in package '{alias}'")

    inputs = iface.get("inputs") or {}
    for topic, entry in (iface.get("outputs") or {}).items():
        replies_to = (entry or {}).get("replies_to")
        if not replies_to:
            continue
        where = f"schema '{name}': interface.outputs.{topic}.replies_to"
        in_entry = inputs.get(replies_to)
        if in_entry is None:
            errors.append(f"{where}: '{replies_to}' is not one of this service's interface.inputs")
            continue
        in_type = (in_entry or {}).get("type") or ""
        out_type = (entry or {}).get("type") or ""
        for label, t in (("request", in_type), ("reply", out_type)):
            if not has_field(universe, t, CORRELATION_FIELD):
                errors.append(f"{where}: {label} type '{t}' has no '{CORRELATION_FIELD}' field")
    return errors


def fail_on(errors: list[str], schema_path: str):
    if errors:
        print(f"❌ Schema validation failed for {schema_path}:")
        for e in errors:
            print(f"   - {e}")
        raise SystemExit(1)


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
                   for p in iter_core_proto_files(proto_dir)]
    proto_packages = []
    for f in proto_files:
        pkg_decl = read_proto_package(os.path.join(proto_dir, f))
        proto_packages.append({"file": pkg_decl, "mod": pkg_decl.split(".")[-1]})

    schema_files = [os.path.join(proto_dir, f)
                     for f in sorted(os.listdir(proto_dir)) if f.endswith(".schema.yaml")]
    modules_dir = os.path.join(proto_dir, "modules")
    if os.path.isdir(modules_dir):
        for name in sorted(os.listdir(modules_dir)):
            p = os.path.join(modules_dir, name, f"{name}.schema.yaml")
            if os.path.exists(p):
                schema_files.append(p)

    services = []
    for schema_path in schema_files:
        with open(schema_path) as sf:
            svc_schema = yaml.safe_load(sf)
        fail_on(validate_core_schema(svc_schema, universe), schema_path)
        iface = svc_schema.get("interface", {}) or {}

        def entries(kind):
            return [
                {
                    "name":      n,
                    "const":     n.upper(),
                    "rust_path": schema_type_to_rust_path((d or {}).get("type") or ""),
                    "targeted":  bool((d or {}).get("targeted", False)),
                }
                for n, d in (iface.get(kind, {}) or {}).items()
            ]

        svc_name = svc_schema.get("name")
        services.append({
            "name":    svc_name,
            "snake":   svc_name.replace("-", "_"),
            "inputs":  entries("inputs"),
            "outputs": entries("outputs"),
        })

    proto_dir_rel = os.path.relpath(proto_dir, out_dir).replace("\\", "/")
    template_data = {
        "package":        package,
        "proto_files":    proto_files,
        "proto_packages": proto_packages,
        "proto_dir_rel":  proto_dir_rel,
        "services":       services,
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
    fail_on(errors, schema_path)

    own_alias = local_package_alias(schema_dir)

    def module_rust_path(canonical: str) -> str:
        """Rust path of a canonical 'alias/Message' inside the module crate."""
        alias, tname = canonical.split("/")
        rust_name = rust_type_name(tname)
        if universe[alias]["origin"] == "core":
            return f"veldsdk::proto::{alias}::{rust_name}"
        return f"crate::proto::{alias}::{rust_name}"

    # ── Build handler dispatch table (topic → handler + payload type) ────────
    handlers = []

    # Handler name = schema key, verbatim (no injected on_input_/on_sub_
    # prefix): the schema is expected to already name it `on_*`, so the
    # topic key and the Rust function it maps to are the same string.
    for input_name in schema.get("interface", {}).get("inputs", {}):
        handlers.append({
            "topic":     f"{name}/{input_name}",
            "handler":   f"crate::module::{input_name}",
            "rust_path": module_rust_path(resolved["inputs"][input_name]),
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
    emits = [
        {
            "name": n,
            "rust_path": module_rust_path(resolved["outputs"][n]),
            "targeted": bool((schema["interface"]["outputs"][n] or {}).get("targeted", False)),
        }
        for n in schema.get("interface", {}).get("outputs", {}) or {}
    ]

    dep_calls = []
    for dep_name, dep_data in schema.get("dependencies", {}).items():
        calls = list((dep_data or {}).get("calls", {}) or {})
        if calls:
            dep_calls.append({
                "service": dep_name,
                "snake": dep_name.replace("-", "_"),
                "methods": [
                    {"name": c, "rust_path": module_rust_path(resolved["calls"][(dep_name, c)])}
                    for c in calls
                ],
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
    proto_paths = []
    dep_protos  = []
    cargo_dependencies = {}

    # 1. Add explicitly defined third-party deps
    for dep_name, dep_val in raw_deps.items():
        cargo_dependencies[dep_name] = yaml_dep_to_toml(dep_val)

    # 2. Add schema-inferred internal dependencies
    dep_wrap_crates = {}   # package alias → (wrap crate snake name, dep dir name)
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

            # Dependency on the generated wrap crate
            cargo_dependencies[api_crate_name] = f'{{ path = "../../{dep_name}/generated/wraps/rust" }}'

            # Extract package name for aliasing
            proto_file = os.path.join(dep_dir, "types.proto")
            if os.path.exists(proto_file):
                pkg = read_proto_package(proto_file)
                if pkg:
                    dep_snake = pkg.split(".")[-1]
                    dep_protos.append({
                        "snake": dep_snake,
                        "api_crate": api_crate_snake,
                    })
                    dep_wrap_crates[dep_snake] = (api_crate_snake, api_crate_name, dep_name)

    # ── Wrap-crate input stubs (typed against the exact message) ─────────────
    # Inputs may be typed with another module's message (e.g. the renderer's
    # UiEventResponse); the wrap crate then depends on that module's wrap.
    wrap_dependencies = {}

    def wrap_rust_path(canonical: str) -> str:
        alias, tname = canonical.split("/")
        rust_name = rust_type_name(tname)
        if alias == own_alias:
            return f"crate::proto::{rust_name}"
        if universe[alias]["origin"] == "core":
            return f"veldsdk::proto::{alias}::{rust_name}"
        crate_snake, crate_name, dep_dir_name = dep_wrap_crates[alias]
        wrap_dependencies[crate_name] = \
            f'{{ path = "../../../../{dep_dir_name}/generated/wraps/rust" }}'
        return f"{crate_snake}::{rust_name}"

    inputs = [
        {"name": n, "rust_path": wrap_rust_path(resolved["inputs"][n])}
        for n in schema.get("interface", {}).get("inputs", {}) or {}
    ]

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
        "inputs":             inputs,
        "dep_calls":          dep_calls,
        "hook_event":         hook_event,
        "has_local_proto":    has_local_proto,
        "local_proto_package": local_proto_package,
        "local_proto_path":   local_proto_path,
        "proto_paths":        proto_paths,
        "include_dirs":       include_dirs,
        "dep_protos":         dep_protos,
        "wrap_dependencies":  wrap_dependencies,
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
    if has_local_proto:
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
