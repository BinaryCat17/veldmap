#!/usr/bin/env python3
"""
VeldMap Build Script

Discovers modules automatically by scanning veldmodules/ for directories
that contain both schema.yaml and config.yaml. Each module is built
independently — no shared Cargo workspace.
"""
import os
import hashlib
import json
import shutil
import subprocess
import sys
import time
import argparse

# ── Helpers ───────────────────────────────────────────────────────────────────

def _load_yaml_scalar(path: str, key: str) -> str | None:
    """Read a top-level scalar value from a YAML file.

    Intentionally avoids pyyaml: build.py runs with system Python before
    the venv is created. Only used for simple top-level string fields
    (package, language) that never require full YAML parsing.
    """
    if not os.path.exists(path):
        return None
    with open(path) as f:
        for line in f:
            stripped = line.strip()
            if stripped.startswith(f"{key}:"):
                return stripped.split(":", 1)[1].strip()
    return None


# Печатать ли каждую запускаемую команду (`--verbose`). По умолчанию нет:
# полные строки cargo с абсолютными путями повторяют то, что уже сказано
# заголовком модуля, и тонут в собственном выводе компилятора. Команда, которая
# упала, печатается всегда — там она и нужна, чтобы её повторить руками.
VERBOSE = False


def run(cmd, cwd=None, env=None):
    """Run a shell command; exit on failure.

    Для cargo `cwd` — не косметика: и `rust-toolchain.toml`, и
    `.cargo/config.toml` он ищет вверх от текущего каталога, а не от
    `--manifest-path`. Запуск из корня проекта (корень намеренно языконейтрален
    и Rust-конфигов не содержит) означал бы сборку не тем toolchain и без
    объявленных rustflags — поэтому cargo всегда зовётся из каталога своего
    манифеста.
    """
    if VERBOSE:
        print(f"-> {' '.join(str(c) for c in cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        print(f"\nFATAL: команда завершилась с кодом {res.returncode}:")
        print(f"  {' '.join(str(c) for c in cmd)}")
        if cwd:
            print(f"  (запущена в {cwd})")
        sys.exit(1)


def elapsed(since: float) -> str:
    """Длительность в том виде, в каком её читают: секунды до минуты, дальше
    минуты с секундами."""
    seconds = time.monotonic() - since
    if seconds < 60:
        return f"{seconds:.1f}s"
    return f"{int(seconds // 60)}m {seconds % 60:04.1f}s"


# ── Project paths ─────────────────────────────────────────────────────────────
PROJECT_ROOT  = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKSPACE_PATH = os.path.join(PROJECT_ROOT, "workspace.yaml")

_modules_val = _load_yaml_scalar(WORKSPACE_PATH, "modules_dir")
if _modules_val:
    _modules_val = _modules_val.strip('"\'')
else:
    _modules_val = "veldmodules"

_plugins_val = _load_yaml_scalar(WORKSPACE_PATH, "plugins_dir")
if _plugins_val:
    _plugins_val = _plugins_val.strip('"\'')
else:
    _plugins_val = "build/plugins"

MODULES_DIR   = os.path.join(PROJECT_ROOT, _modules_val)
PLUGINS_DIR   = os.path.join(PROJECT_ROOT, _plugins_val)
RUNTIME_DIR   = os.path.join(PROJECT_ROOT, "runtime")
WASM_TARGET   = "wasm32-wasip1"
CORE_MANIFEST = os.path.join(PROJECT_ROOT, "veldcore", "Cargo.toml")


def _check_plugins_dir_consistency():
    """Сверяет plugins_dir сборки (workspace.yaml) с рантайм-манифестом.

    Хост резолвит plugins_dir из runtime/config/services.json относительно
    runtime/ (см. load_host_config). Раньше эти два объявления одного каталога
    жили независимо, и правка workspace.yaml молча рассинхронизировала сборку
    и запуск: wasm ложились в одно место, а хост искал в другом.
    """
    manifest_path = os.path.join(RUNTIME_DIR, "config", "services.json")
    try:
        with open(manifest_path, encoding="utf-8") as f:
            manifest = json.load(f)
    except (OSError, json.JSONDecodeError):
        manifest = {}
    runtime_value = manifest.get("plugins_dir", "../build/plugins")
    runtime_plugins = os.path.normpath(os.path.join(RUNTIME_DIR, runtime_value))
    if runtime_plugins != os.path.normpath(PLUGINS_DIR):
        sys.exit(
            "plugins_dir расходится между сборкой и рантаймом:\n"
            f"  workspace.yaml: {_plugins_val} -> {os.path.normpath(PLUGINS_DIR)}\n"
            f"  services.json:  {runtime_value} -> {runtime_plugins}\n"
            "Приведите оба к одному каталогу (база workspace.yaml — корень "
            "проекта, база services.json — runtime/)."
        )


def discover_modules() -> list[dict]:
    """Scan veldmodules/ for module directories, sorted in dependency order.

    A directory is a module when it contains both:
      - schema.yaml  (module interface definition)
      - config.yaml  (language + build config)

    Modules that are depended upon (via path deps in config.yaml) are returned
    before the modules that depend on them (topological sort).
    """
    raw = []
    for name in sorted(os.listdir(MODULES_DIR)):
        module_dir = os.path.join(MODULES_DIR, name)
        if not os.path.isdir(module_dir):
            continue
        schema_path = os.path.join(module_dir, "schema.yaml")
        config_path = os.path.join(module_dir, "config.yaml")
        if not os.path.exists(schema_path) or not os.path.exists(config_path):
            continue
        raw.append({
            "name":     name,
            "package":  _load_yaml_scalar(config_path, "package") or name,
            "language": _load_yaml_scalar(config_path, "language") or "rust",
            "dir":      module_dir,
        })

    return _topo_sort(raw)


def _schema_dependencies(modules: list[dict]) -> dict:
    """Read every module's `dependencies:` block with a real YAML parser.

    Runs under the buildgen venv interpreter: build.py itself runs on system
    Python, where pyyaml is not guaranteed (see ensure_venv)."""
    build_dir = os.path.dirname(os.path.abspath(__file__))
    script    = os.path.join(build_dir, "schema_deps.py")
    schemas   = [os.path.join(m["dir"], "schema.yaml") for m in modules]
    if not schemas:
        return {}
    try:
        out = subprocess.check_output([ensure_venv(), script] + schemas, text=True)
    except subprocess.CalledProcessError as e:
        sys.exit(f"FATAL: cannot parse module schemas:\n{e.output or e}")
    return json.loads(out)


def _topo_sort(modules: list[dict]) -> list[dict]:
    """Return modules in build order: dependencies before dependents.

    Dependency edges come from the `dependencies:` block of each module's
    schema.yaml: if module A is listed among module B's dependencies,
    A must build first.
    """
    # Map: module name -> module dict
    by_name = {m["name"]: m for m in modules}
    declared = _schema_dependencies(modules)

    # Build adjacency: name -> set of names it depends on
    deps: dict[str, set] = {m["name"]: set() for m in modules}
    for m in modules:
        for dep_name in declared.get(os.path.join(m["dir"], "schema.yaml"), []):
            if dep_name in by_name:
                deps[m["name"]].add(dep_name)

    # Kahn's algorithm: in_degree[name] = число зависимостей, которые должны
    # собраться раньше name.
    in_degree = {name: 0 for name in by_name}
    for _name, dep_set in deps.items():
        for _dep in dep_set:
            in_degree[_name] += 1

    queue  = [n for n in by_name if in_degree[n] == 0]
    result = []
    while queue:
        queue.sort()  # deterministic order within same level
        node = queue.pop(0)
        result.append(by_name[node])
        for name, dep_set in deps.items():
            if node in dep_set:
                in_degree[name] -= 1
                if in_degree[name] == 0:
                    queue.append(name)

    if len(result) != len(modules):
        print("WARNING: Circular dependency detected in modules, using original order.")
        return modules

    return result



# ── Code generation ───────────────────────────────────────────────────────────

def ensure_venv() -> str:
    """Сборочный venv из requirements.txt; возвращает путь к его python.

    Состав окружения сверяется с requirements.txt по хэшу, а не только по факту
    существования каталога: иначе новая зависимость доезжала бы только до тех,
    кто собирает с нуля, а у всех остальных сборка падала бы импортом.
    """
    build_dir    = os.path.dirname(os.path.abspath(__file__))
    venv_dir     = os.path.join(build_dir, ".venv")
    venv_python  = os.path.join(venv_dir, "bin", "python")
    requirements = os.path.join(build_dir, "requirements.txt")
    stamp        = os.path.join(venv_dir, ".requirements-sha256")

    with open(requirements, "rb") as f:
        want = hashlib.sha256(f.read()).hexdigest()

    if not os.path.exists(venv_python):
        print("Initializing build venv...")
        run(["python3", "-m", "venv", ".venv"], cwd=build_dir)
    elif os.path.exists(stamp):
        with open(stamp) as f:
            if f.read().strip() == want:
                return venv_python

    print("Installing build dependencies...")
    run([venv_python, "-m", "pip", "install", "-q", "-r", requirements])
    with open(stamp, "w") as f:
        f.write(want)
    return venv_python


def check_generator():
    """Прогнать тесты кодогенератора перед кодогенерацией.

    Валидатор схем — единственное, что не даёт схеме разойтись с кодом, и
    ломается он молча: ослабевшая проверка не мешает сборке пройти, а
    расхождение всплывает уже в рантайме недоставленным событием. Тесты идут
    первым шагом и стоят десятые доли секунды — дешевле, чем один неверно
    собранный модуль.
    """
    build_dir = os.path.dirname(os.path.abspath(__file__))
    print("\nChecking code generator...")
    run([ensure_venv(), "-m", "pytest", "-q", "--no-header",
         os.path.join(build_dir, "tests")], cwd=build_dir)


def generate_code():
    """Run generate.py for every discovered module (using absolute paths)."""
    print("\n[1/4] Generating module bindings...")
    started     = time.monotonic()
    build_dir   = os.path.dirname(os.path.abspath(__file__))
    venv_python = ensure_venv()
    gen_script  = os.path.join(build_dir, "generate.py")
    # Генератор отчитывается об успехе сам; при обычной сборке это шесть строк
    # об одном и том же, поэтому он молчит, а имена перечисляются одной строкой.
    quiet       = [] if VERBOSE else ["--quiet"]

    generated = []
    for module in discover_modules():
        schema_path   = os.path.join(module["dir"], "schema.yaml")
        generated_dir = os.path.join(module["dir"], "generated")
        run([venv_python, gen_script,
             "--schema",     schema_path,
             "--output-dir", generated_dir] + quiet)
        generated.append(module["name"])

    # Платформенные сервисы (app, tasks, ...) для wasm-модулей стабов в SDK не
    # имеют: модуль объявляет их в своей schema.yaml как обычную зависимость
    # (`app: calls: [on_set_surface]`), и стабы приходят через crate::calls::*
    # вместе со всеми остальными. Иначе исходящая связь была бы невидима для
    # валидатора схем и для графа зависимостей.

    # Хостовые биндинги (veldcore/platform/host/host.yaml → platform/host/generated)
    host_dir  = os.path.join(PROJECT_ROOT, "veldcore", "platform", "host")
    host_yaml = os.path.join(host_dir, "host.yaml")
    if os.path.exists(host_yaml):
        host_lang = _load_yaml_scalar(host_yaml, "language") or "rust"
        host_pkg  = _load_yaml_scalar(host_yaml, "package") or "veldmap-host-bindings"
        if host_lang == "rust":
            run([venv_python, gen_script,
                 "--host-bindings", os.path.join(host_dir, "generated"),
                 "--proto-dir",     os.path.join(PROJECT_ROOT, "veldcore", "interface"),
                 "--package",       host_pkg] + quiet)
            generated.append("host bindings")
        else:
            print(f"  WARNING: no host bindings generator for language '{host_lang}', skipping")

    print(f"  {', '.join(generated)} — {elapsed(started)}")


# ── Module builders (one per language) ────────────────────────────────────────

_WASM_CC_ENV: dict | None = None

def wasm_cc_env() -> dict:
    """Окружение cc-rs для C-зависимостей wasm-модулей (zstd-sys у image-tiler).

    `cc` берёт компилятор из PATH, а первый найденный clang бывает без
    бэкенда wasm32 (наборы вроде esp-clang). Кандидаты пробуются один раз за
    сборку; уже выставленные переменные cc-rs уважаются. Не нашлось —
    печатается предупреждение, и C-зависимость откажет своей ошибкой:
    чисто-Rust модулям компилятор не нужен вовсе.
    """
    global _WASM_CC_ENV
    if _WASM_CC_ENV is not None:
        return _WASM_CC_ENV
    target = WASM_TARGET.replace("-", "_")
    if os.environ.get(f"CC_{target}") or os.environ.get(f"CC_{WASM_TARGET}"):
        _WASM_CC_ENV = {}
        return _WASM_CC_ENV

    env = {}
    for cc in ("clang", "/usr/bin/clang", "clang-19", "clang-18", "clang-17"):
        try:
            targets = subprocess.check_output(
                [cc, "--print-targets"], text=True, stderr=subprocess.DEVNULL)
        except (OSError, subprocess.CalledProcessError):
            continue
        if "wasm32" in targets:
            env[f"CC_{target}"] = cc
            # Без freestanding заголовки clang в hosted-режиме дотягиваются
            # include_next'ом до хостового glibc, которого у wasm-цели нет;
            # libc-часть докрывает wasm-shim самого zstd-sys.
            env[f"CFLAGS_{target}"] = "-ffreestanding"
            break
    if env:
        # Архиватор статических библиотек — llvm-ar: бинутилсовый ar про
        # wasm-объекты не знает.
        for ar in ("llvm-ar", "llvm-ar-19", "llvm-ar-18", "llvm-ar-17"):
            if shutil.which(ar):
                env[f"AR_{target}"] = ar
                break
    else:
        print("  WARNING: clang с бэкендом wasm32 не найден — "
              "C-зависимости wasm-модулей не соберутся")
    _WASM_CC_ENV = env
    return env


def build_rust_module(module: dict, profile: str, cargo_args: list):
    """Build a standalone Rust WASM module and deploy to PLUGINS_DIR."""
    package       = module["package"]
    generated_dir = os.path.join(module["dir"], "generated")
    manifest      = os.path.join(generated_dir, "Cargo.toml")

    extra = wasm_cc_env()
    env = {**os.environ, **extra} if extra else None

    run(["cargo", "build",
         "--manifest-path", manifest,
         "-p", package,
         "--target", WASM_TARGET,
         ] + cargo_args, cwd=generated_dir, env=env)

    # Wrap-крейт листового модуля ни от кого не зависит и без явной проверки
    # никогда не компилируется — ошибки в его typed-стабах молчали бы.
    wrap_manifest = os.path.join(generated_dir, "wraps", "rust", "Cargo.toml")
    if os.path.exists(wrap_manifest):
        run(["cargo", "check",
             "--manifest-path", wrap_manifest,
             "--target", WASM_TARGET,
             ] + cargo_args, cwd=os.path.dirname(wrap_manifest), env=env)

    wasm_name   = package.replace("-", "_") + ".wasm"
    source_path = os.path.join(generated_dir, "target", WASM_TARGET, profile, wasm_name)
    dest_path   = os.path.join(PLUGINS_DIR, wasm_name)

    shutil.copy(source_path, dest_path)
    return wasm_name


# ── Main build ─────────────────────────────────────────────────────────────────

def build_all(debug: bool = False, windows: bool = False, dist_dir: str | None = None):
    """Generate bindings, build all WASM modules, then build the native host."""
    profile    = "debug" if debug else "release"
    cargo_args = [] if debug else ["--release"]

    started = time.monotonic()
    check_generator()
    generate_code()

    # 2. WASM modules
    print(f"\n[2/4] Building WASM modules ({profile})...")
    os.makedirs(PLUGINS_DIR, exist_ok=True)

    BUILDERS = {
        "rust": build_rust_module,
        # "go":   build_go_module,  ← extend here for new languages
    }

    for module in discover_modules():
        lang    = module["language"]
        builder = BUILDERS.get(lang)
        if not builder:
            print(f"  WARNING: no builder for language '{lang}', skipping {module['name']}")
            continue
        print(f"\n--- {module['name']} ({lang}) ---")
        module_started = time.monotonic()
        artefact = builder(module, profile, cargo_args)
        print(f"  {artefact} -> build/plugins/ — {elapsed(module_started)}")

    # 3. Native host
    print(f"\n[3/4] Building native host ({profile})...")
    host_started = time.monotonic()
    host_args = list(cargo_args)
    if windows:
        host_args += ["--target", "x86_64-pc-windows-gnu"]

    run(["cargo", "build",
         "--manifest-path", CORE_MANIFEST,
         "-p", "veldmap-host-gui",
         ] + host_args, cwd=os.path.dirname(CORE_MANIFEST))
    print(f"  готов за {elapsed(host_started)}")

    # 4. Юнит-тесты чистой логики (нативные)
    test_rust_units()

    if windows:
        _deploy_windows(profile, dist_dir)

    return started


def _has_unit_tests(module: dict) -> bool:
    """Есть ли в исходниках модуля юнит-тесты (`#[cfg(test)]`)."""
    src = os.path.join(module["dir"], "src")
    for root, _dirs, files in os.walk(src):
        for name in files:
            if not name.endswith(".rs"):
                continue
            with open(os.path.join(root, name), encoding="utf-8") as f:
                if "#[cfg(test)]" in f.read():
                    return True
    return False


def test_rust_units():
    """Юнит-тесты чистой логики: календарь, кодек сообщений, геодезия, Dedup.

    Компилируются и бегут нативно — хоста им не нужно, хостовые ABI-функции
    подменяются заглушками SDK (см. veldsdk::abi, cfg(not(target_arch =
    "wasm32"))). Это единственная механическая проверка Rust-кода помимо
    «компилируется»: интерфейс по-прежнему проверяется запуском (см. CLAUDE.md),
    но парные функции — encode/decode, прямая/обратная — сходятся здесь.

    `--release`, чтобы у veldcore тесты переиспользовали кэш обычной сборки.
    Модули гоняются только те, где тесты есть: остальным незачем компилировать
    нативный вариант крейта.
    """
    print("\n[4/4] Rust unit tests...")
    started = time.monotonic()

    run(["cargo", "test", "--release", "-q", "-p", "veldsdk", "-p", "veldmap-host-core"],
        cwd=os.path.join(PROJECT_ROOT, "veldcore"))

    tested = ["veldsdk", "veldmap-host-core"]
    for module in discover_modules():
        if module["language"] != "rust" or not _has_unit_tests(module):
            continue
        run(["cargo", "test", "--release", "-q", "-p", module["package"]],
            cwd=os.path.join(module["dir"], "generated"))
        tested.append(module["name"])

    print(f"  {', '.join(tested)} — {elapsed(started)}")


# ── Windows deployment ─────────────────────────────────────────────────────────

def _deploy_windows(profile: str, dist_dir: str | None):
    if dist_dir is None:
        print("ERROR: --dist-dir is required for --windows deployment.")
        sys.exit(1)

    gui_exe      = os.path.join(PROJECT_ROOT, "veldcore", "target",
                                "x86_64-pc-windows-gnu", profile, "veldmap-host-gui.exe")
    config_dir   = os.path.join(RUNTIME_DIR, "config")
    dist_plugins = os.path.join(dist_dir, "plugins")
    dist_config  = os.path.join(dist_dir, "config")

    print(f"\n[Deploy] -> {dist_dir}")
    for d in [dist_dir, dist_plugins, dist_config]:
        os.makedirs(d, exist_ok=True)

    shutil.copy(gui_exe, os.path.join(dist_dir, "veldmap-host-gui.exe"))

    for wasm in os.listdir(PLUGINS_DIR):
        if wasm.endswith(".wasm"):
            shutil.copy(os.path.join(PLUGINS_DIR, wasm), os.path.join(dist_plugins, wasm))

    if os.path.isdir(config_dir):
        for cfg in os.listdir(config_dir):
            if cfg.endswith(".json"):
                shutil.copy(os.path.join(config_dir, cfg), os.path.join(dist_config, cfg))

    env_file = os.path.join(PROJECT_ROOT, ".env")
    if os.path.exists(env_file):
        print("  Deploying .env ...")
        shutil.copy(env_file, os.path.join(dist_dir, ".env"))

    print(f"Windows x64 build deployed to: {dist_dir}")


# ── Clean ──────────────────────────────────────────────────────────────────────

def clean():
    """Remove all build artefacts."""
    print("Cleaning project...")
    targets = [
        os.path.join(PROJECT_ROOT, "veldcore", "target"),
        PLUGINS_DIR,
    ]

    for module in discover_modules():
        targets.append(os.path.join(module["dir"], "generated", "target"))

    targets.append(os.path.join(PROJECT_ROOT, "veldcore", "platform", "host", "generated", "target"))

    for folder in targets:
        if os.path.exists(folder):
            print(f"  Removing {folder}/")
            shutil.rmtree(folder)
    print("Done.")


# ── Entry point ────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="VeldMap Build Script")
    parser.add_argument("command", choices=["build", "clean"],
                        nargs="?", default="build",
                        help="Command to run (default: build)")
    parser.add_argument("--debug",    action="store_true",
                        help="Build in debug mode")
    parser.add_argument("--windows",  action="store_true",
                        help="Cross-compile for Windows x86_64")
    parser.add_argument("--dist-dir", default=None,
                        help="Windows deployment directory (required with --windows)")
    parser.add_argument("--verbose", "-v", action="store_true",
                        help="Печатать каждую запускаемую команду")
    args = parser.parse_args()

    # Подпроцессы (cargo, pytest, generate.py) пишут в тот же дескриптор
    # напрямую, а print() в питоне при перенаправлении в файл буферизуется
    # блоками — и лог сборки выходил перемешанным: сначала чужой вывод, потом
    # наши заголовки. В терминале этого не видно, в `> build.log` видно сразу.
    sys.stdout.reconfigure(line_buffering=True)

    global VERBOSE
    VERBOSE = args.verbose

    if args.command == "clean":
        clean()
    else:
        _check_plugins_dir_consistency()
        started = build_all(debug=args.debug, windows=args.windows, dist_dir=args.dist_dir)
        mode   = "DEBUG" if args.debug else "RELEASE"
        target = " (Windows x64)" if args.windows else ""
        print(f"\n{'='*45}")
        print(f"{mode}{target} build complete in {elapsed(started)}")
        print(f"{'='*45}\n")


if __name__ == "__main__":
    main()