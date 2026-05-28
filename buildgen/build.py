#!/usr/bin/env python3
"""
VeldMap Build Script

Discovers modules automatically by scanning veldmodules/ for directories
that contain both schema.yaml and config.yaml. Each module is built
independently — no shared Cargo workspace.
"""
import os
import shutil
import subprocess
import sys
import argparse

# ── Project paths ─────────────────────────────────────────────────────────────
PROJECT_ROOT  = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MODULES_DIR   = os.path.join(PROJECT_ROOT, "veldmodules")
PLUGINS_DIR   = os.path.join(PROJECT_ROOT, "build", "plugins")
RUNTIME_DIR   = os.path.join(PROJECT_ROOT, "runtime")
WASM_TARGET   = "wasm32-wasip1"
CORE_MANIFEST = os.path.join(PROJECT_ROOT, "veldcore", "Cargo.toml")

# ── Helpers ───────────────────────────────────────────────────────────────────

def run(cmd, cwd=None, env=None):
    """Run a shell command; exit on failure."""
    print(f"-> {' '.join(str(c) for c in cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        print(f"\nFATAL: Command failed with exit code {res.returncode}")
        sys.exit(1)


def _load_yaml_scalar(path: str, key: str) -> str | None:
    """Read a top-level scalar value from a YAML file.

    Intentionally avoids pyyaml: build.py runs with system Python before
    the venv is created. Only used for simple top-level string fields
    (package, language) that never require full YAML parsing.
    """
    with open(path) as f:
        for line in f:
            stripped = line.strip()
            if stripped.startswith(f"{key}:"):
                return stripped.split(":", 1)[1].strip()
    return None


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


def _topo_sort(modules: list[dict]) -> list[dict]:
    """Return modules in build order: dependencies before dependents.

    Dependency edges are inferred from 'path' entries in config.yaml:
    if module A's generated/ is referenced by module B, A must build first.
    """
    # Map: module name -> module dict
    by_name = {m["name"]: m for m in modules}

    # Build adjacency: name -> set of names it depends on
    deps: dict[str, set] = {m["name"]: set() for m in modules}
    for m in modules:
        schema_path = os.path.join(m["dir"], "schema.yaml")
        # Simple scan: look for modules defined in `dependencies:` block
        try:
            with open(schema_path) as f:
                in_deps = False
                for line in f:
                    stripped = line.strip()
                    if not stripped or stripped.startswith("#"):
                        continue
                    if line.startswith("dependencies:"):
                        in_deps = True
                        continue
                    if in_deps:
                        if not line.startswith(" ") and not line.startswith("\t"):
                            in_deps = False
                            continue
                        # Direct children of dependencies block
                        if line.startswith("  ") and not line.startswith("   ") and ":" in stripped:
                            dep_name = stripped.split(":")[0].strip()
                            if dep_name in by_name:
                                deps[m["name"]].add(dep_name)
        except Exception:
            pass

    # Kahn's algorithm
    in_degree = {name: 0 for name in by_name}
    for name, dep_set in deps.items():
        for dep in dep_set:
            in_degree[name] = in_degree.get(name, 0)
            # name depends on dep → dep must come first → name's in_degree++
    # Recompute properly
    in_degree = {name: 0 for name in by_name}
    for name, dep_set in deps.items():
        for dep in dep_set:
            in_degree[name] += 1

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
    """Create the buildgen venv if missing; return path to its Python binary."""
    build_dir  = os.path.dirname(os.path.abspath(__file__))
    venv_python = os.path.join(build_dir, ".venv", "bin", "python")
    if not os.path.exists(venv_python):
        print("Initializing build venv...")
        run(["python3", "-m", "venv", ".venv"], cwd=build_dir)
        run([venv_python, "-m", "pip", "install", "pyyaml", "jinja2"])
    return venv_python


def generate_code():
    """Run generate.py for every discovered module (using absolute paths)."""
    print("\n[0/2] Generating module bindings...")
    build_dir   = os.path.dirname(os.path.abspath(__file__))
    venv_python = ensure_venv()
    gen_script  = os.path.join(build_dir, "generate.py")

    for module in discover_modules():
        schema_path   = os.path.join(module["dir"], "schema.yaml")
        generated_dir = os.path.join(module["dir"], "generated")
        print(f"  Generating {module['name']} ...")
        run([venv_python, gen_script,
             "--schema",     schema_path,
             "--output-dir", generated_dir])


# ── Module builders (one per language) ────────────────────────────────────────

def build_rust_module(module: dict, profile: str, cargo_args: list):
    """Build a standalone Rust WASM module and deploy to PLUGINS_DIR."""
    package       = module["package"]
    generated_dir = os.path.join(module["dir"], "generated")
    manifest      = os.path.join(generated_dir, "Cargo.toml")

    run(["cargo", "build",
         "--manifest-path", manifest,
         "-p", package,
         "--target", WASM_TARGET,
         ] + cargo_args)

    wasm_name   = package.replace("-", "_") + ".wasm"
    source_path = os.path.join(generated_dir, "target", WASM_TARGET, profile, wasm_name)
    dest_path   = os.path.join(PLUGINS_DIR, wasm_name)

    print(f"  Deploying {wasm_name} -> build/plugins/")
    shutil.copy(source_path, dest_path)


# ── Main build ─────────────────────────────────────────────────────────────────

def build_all(debug: bool = False, windows: bool = False, dist_dir: str | None = None):
    """Generate bindings, build all WASM modules, then build the native host."""
    profile    = "debug" if debug else "release"
    cargo_args = [] if debug else ["--release"]

    generate_code()

    # 1. WASM modules
    print(f"\n[1/2] Building WASM modules ({profile})...")
    os.makedirs(PLUGINS_DIR, exist_ok=True)

    BUILDERS = {
        "rust": build_rust_module,
        # "go":   build_go_module,  ← extend here for new languages
    }

    for module in discover_modules():
        lang    = module["language"]
        builder = BUILDERS.get(lang)
        print(f"\n--- {module['name']} ({lang}) ---")
        if builder:
            builder(module, profile, cargo_args)
        else:
            print(f"  WARNING: no builder for language '{lang}', skipping")

    # 2. Native host
    print(f"\n[2/2] Building native host ({profile})...")
    host_args = list(cargo_args)
    if windows:
        host_args += ["--target", "x86_64-pc-windows-gnu"]

    run(["cargo", "build",
         "--manifest-path", CORE_MANIFEST,
         "-p", "veldmap-host-gui",
         ] + host_args)

    if windows:
        _deploy_windows(profile, dist_dir)


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
    args = parser.parse_args()

    if args.command == "clean":
        clean()
    else:
        build_all(debug=args.debug, windows=args.windows, dist_dir=args.dist_dir)
        mode   = "DEBUG" if args.debug else "RELEASE"
        target = " (Windows x64)" if args.windows else ""
        print(f"\n{'='*35}")
        print(f"{mode}{target} build complete!")
        print(f"{'='*35}\n")


if __name__ == "__main__":
    main()