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

# ── Project paths ────────────────────────────────────────────────────────────
MODULES_DIR      = "veldmodules"
PLUGINS_DIR      = "veldmodules/plugins"
WASM_TARGET      = "wasm32-wasip1"
CORE_MANIFEST    = "veldcore/Cargo.toml"
WINDOWS_DIST_DIR = "/mnt/c/Users/smirn/Documents/veldmap/build"

# ── Helpers ──────────────────────────────────────────────────────────────────

def run(cmd, cwd=None, env=None):
    """Run a shell command; exit on failure."""
    print(f"-> {' '.join(str(c) for c in cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        print(f"\nFATAL: Command failed with exit code {res.returncode}")
        sys.exit(1)


def _yaml_scalar(path, key):
    """Read a top-level 'key: value' from a simple YAML file without pyyaml."""
    with open(path) as f:
        for line in f:
            stripped = line.strip()
            if stripped.startswith(f"{key}:"):
                return stripped.split(":", 1)[1].strip()
    return None


def discover_modules():
    """
    Scan veldmodules/ for module directories.

    A directory is considered a module when it contains both:
      - schema.yaml  (module interface definition)
      - config.yaml  (language + build config)
    """
    modules = []
    for name in sorted(os.listdir(MODULES_DIR)):
        module_dir = os.path.join(MODULES_DIR, name)
        if not os.path.isdir(module_dir):
            continue
        schema_path = os.path.join(module_dir, "schema.yaml")
        config_path = os.path.join(module_dir, "config.yaml")
        if not os.path.exists(schema_path) or not os.path.exists(config_path):
            continue
        modules.append({
            "name":      name,
            "package":   _yaml_scalar(config_path, "package") or name,
            "language":  _yaml_scalar(config_path, "language") or "rust",
            "dir":       module_dir,
        })
    return modules

# ── Code generation ──────────────────────────────────────────────────────────

def generate_code():
    """Run generate.py for every discovered module."""
    print("\n[0/2] Generating module bindings...")
    build_dir = os.path.dirname(os.path.abspath(__file__))
    venv_python = os.path.join(build_dir, ".venv", "bin", "python")
    gen_script  = os.path.join(build_dir, "generate.py")

    if not os.path.exists(venv_python):
        print("Initializing build venv...")
        run(["python3", "-m", "venv", ".venv"], cwd=build_dir)
        run([venv_python, "-m", "pip", "install", "pyyaml", "jinja2"])

    for module in discover_modules():
        schema_path   = os.path.join(module["dir"], "schema.yaml")
        generated_dir = os.path.join(module["dir"], "generated")
        print(f"  Generating {module['name']} ...")
        run(
            [venv_python, gen_script,
             "--schema",     f"../{schema_path}",
             "--output-dir", f"../{generated_dir}"],
            cwd=build_dir,
        )

# ── Module builders (one per language) ───────────────────────────────────────

def build_rust_module(module, profile, cargo_args):
    """Build a standalone Rust WASM module."""
    package       = module["package"]
    generated_dir = os.path.join(module["dir"], "generated")
    manifest      = os.path.join(generated_dir, "Cargo.toml")

    run(["cargo", "build",
         "--manifest-path", manifest,
         "-p", package,
         "--target", WASM_TARGET,
         ] + cargo_args)

    # Each module owns its own target/ next to its Cargo.toml
    wasm_name   = package.replace("-", "_") + ".wasm"
    source_path = os.path.join(generated_dir, "target", WASM_TARGET, profile, wasm_name)
    dest_path   = os.path.join(PLUGINS_DIR, wasm_name)

    print(f"  Deploying {wasm_name} -> {PLUGINS_DIR}/")
    shutil.copy(source_path, dest_path)

# ── Main build ────────────────────────────────────────────────────────────────

def build_all(debug=False, windows=False):
    """Generate bindings, build all WASM modules, then build the native host."""
    profile     = "debug" if debug else "release"
    cargo_args  = [] if debug else ["--release"]

    generate_code()

    # ── 1. WASM modules ──────────────────────────────────────────────────────
    print(f"\n[1/2] Building WASM modules ({profile})...")
    os.makedirs(PLUGINS_DIR, exist_ok=True)

    BUILDERS = {
        "rust": build_rust_module,
        # "go":   build_go_module,   ← extend here for new languages
    }

    for module in discover_modules():
        lang    = module["language"]
        builder = BUILDERS.get(lang)
        print(f"\n--- {module['name']} ({lang}) ---")
        if builder:
            builder(module, profile, cargo_args)
        else:
            print(f"  WARNING: no builder for language '{lang}', skipping")

    # ── 2. Native host ───────────────────────────────────────────────────────
    print(f"\n[2/2] Building native host ({profile})...")
    host_args = list(cargo_args)
    if windows:
        host_args += ["--target", "x86_64-pc-windows-gnu"]

    run(["cargo", "build",
         "--manifest-path", CORE_MANIFEST,
         "-p", "veldmap-host-gui",
         ] + host_args)

    if windows:
        _deploy_windows(profile)

# ── Windows deployment ────────────────────────────────────────────────────────

def _deploy_windows(profile):
    gui_exe     = os.path.join("veldcore/target", "x86_64-pc-windows-gnu", profile, "veldmap-host-gui.exe")
    config_dir  = os.path.join(MODULES_DIR, "config")
    dist_plugins = os.path.join(WINDOWS_DIST_DIR, "plugins")
    dist_config  = os.path.join(WINDOWS_DIST_DIR, "config")

    print(f"\n[Deploy] -> {WINDOWS_DIST_DIR}")
    for d in [WINDOWS_DIST_DIR, dist_plugins, dist_config]:
        os.makedirs(d, exist_ok=True)

    shutil.copy(gui_exe, os.path.join(WINDOWS_DIST_DIR, "veldmap-host-gui.exe"))

    for wasm in os.listdir(PLUGINS_DIR):
        if wasm.endswith(".wasm"):
            shutil.copy(os.path.join(PLUGINS_DIR, wasm), os.path.join(dist_plugins, wasm))

    if os.path.isdir(config_dir):
        for cfg in os.listdir(config_dir):
            if cfg.endswith(".json"):
                shutil.copy(os.path.join(config_dir, cfg), os.path.join(dist_config, cfg))

    if os.path.exists(".env"):
        print("  Deploying .env ...")
        shutil.copy(".env", os.path.join(WINDOWS_DIST_DIR, ".env"))

    print(f"Windows x64 build deployed to: {WINDOWS_DIST_DIR}")

# ── Clean ─────────────────────────────────────────────────────────────────────

def clean():
    """Remove all build artefacts."""
    print("Cleaning project...")
    targets = ["veldcore/target", PLUGINS_DIR]

    # Each module has its own target/ inside generated/
    for module in discover_modules():
        t = os.path.join(module["dir"], "generated", "target")
        targets.append(t)

    for folder in targets:
        if os.path.exists(folder):
            print(f"  Removing {folder}/")
            shutil.rmtree(folder)
    print("Done.")

# ── Entry point ───────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="VeldMap Build Script")
    parser.add_argument("command", choices=["build", "clean"],
                        nargs="?", default="build",
                        help="Command to run (default: build)")
    parser.add_argument("--debug",   action="store_true", help="Build in debug mode")
    parser.add_argument("--windows", action="store_true", help="Cross-compile for Windows x86_64")
    args = parser.parse_args()

    if args.command == "clean":
        clean()
    else:
        build_all(debug=args.debug, windows=args.windows)
        mode   = "DEBUG" if args.debug else "RELEASE"
        target = " (Windows x64)" if args.windows else ""
        print(f"\n{'='*35}")
        print(f"{mode}{target} build complete!")
        print(f"{'='*35}\n")


if __name__ == "__main__":
    main()