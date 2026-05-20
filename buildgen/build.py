#!/usr/bin/env python3
import os
import shutil
import subprocess
import sys
import argparse

# Project configuration
PLUGINS_DIR = "veldgis/plugins"
CONFIG_DIR = "veldgis/config"
WINDOWS_DIST_DIR = "/mnt/c/Users/smirn/Documents/veldmap/build"
WASM_TARGET = "wasm32-wasip1"
CORE_MANIFEST = "veldcore/Cargo.toml"
GIS_MANIFEST = "veldgis/Cargo.toml"
STD_MANIFEST = "veldstd/Cargo.toml"

# Format: (name, workspace_manifest, target_dir, schema_path, generated_dir)
MODULES = [
    ("veldmap-data-provider", "veldgis/modules/data-provider/generated/Cargo.toml", "veldgis/target", "veldgis/modules/data-provider/schema.yaml", "veldgis/modules/data-provider/generated"),
    ("veldmap-data-browser", "veldgis/apps/data-browser/generated/Cargo.toml", "veldgis/target", "veldgis/apps/data-browser/schema.yaml", "veldgis/apps/data-browser/generated"),
    ("veld-ui-service", "veldstd/ui-service/module/generated/Cargo.toml", "veldstd/target", "veldstd/ui-service/module/schema.yaml", "veldstd/ui-service/module/generated"),
]

def run(cmd, cwd=None, env=None):
    """Run a shell command and exit on failure."""
    print(f"-> {' '.join(cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        print(f"\nFATAL: Command failed with exit code {res.returncode}")
        sys.exit(1)

def generate_code():
    """Run the code generator for modules with schemas."""
    print("\n[0/2] Generating module bindings...")
    build_dir = os.path.dirname(os.path.abspath(__file__))
    venv_python = os.path.join(build_dir, ".venv", "bin", "python")
    gen_script = os.path.join(build_dir, "generate.py")
    
    if not os.path.exists(venv_python):
        print("Initializing build venv...")
        run(["python3", "-m", "venv", ".venv"], cwd=build_dir)
        run([venv_python, "-m", "pip", "install", "pyyaml", "jinja2"])

    for module, manifest, target_dir, schema_path, generated_dir in MODULES:
        if schema_path and generated_dir:
            print(f"Generating {module} from {schema_path}...")
            run([venv_python, gen_script, "--schema", f"../{schema_path}", "--output-dir", f"../{generated_dir}"], cwd=build_dir)

def build_all(debug=False, windows=False):
    """Build WASM modules and Host."""
    profile = "debug" if debug else "release"
    cargo_args = [] if debug else ["--release"]
    
    generate_code()
    
    # 1. Build WASM Modules
    print(f"\n[1/2] Building WASM Modules ({profile})...")
    if not os.path.exists(PLUGINS_DIR):
        os.makedirs(PLUGINS_DIR)

    for module, manifest, target_dir, schema, gen_dir in MODULES:
        print(f"\n--- Module: {module} ---")
        cmd = ["cargo", "build", "--manifest-path", manifest, "-p", module, "--target", WASM_TARGET] + cargo_args
        run(cmd)
        
        # Rust lib names use underscores instead of hyphens
        wasm_file_name = module.replace("-", "_") + ".wasm"
        source_path = os.path.join(target_dir, WASM_TARGET, profile, wasm_file_name)
        dest_path = os.path.join(PLUGINS_DIR, wasm_file_name)
        
        print(f"Deploying {wasm_file_name} to {PLUGINS_DIR}/")
        shutil.copy(source_path, dest_path)

    # 2. Build Native Hosts (in Core workspace)
    print(f"\n[2/2] Building Native Host (GUI) ({profile})...")
    host_args = list(cargo_args)
    if windows:
        host_args += ["--target", "x86_64-pc-windows-gnu"]
    
    run(["cargo", "build", "--manifest-path", CORE_MANIFEST, "-p", "veldmap-host-gui"] + host_args)

    if windows:
        gui_exe = os.path.join("veldcore/target", "x86_64-pc-windows-gnu", profile, "veldmap-host-gui.exe")
        
        print(f"\n[Deploy] Deploying to {WINDOWS_DIST_DIR}...")
        
        # Create directory structure
        dist_plugins = os.path.join(WINDOWS_DIST_DIR, "plugins")
        dist_config = os.path.join(WINDOWS_DIST_DIR, "config")
        
        for d in [WINDOWS_DIST_DIR, dist_plugins, dist_config]:
            if not os.path.exists(d):
                os.makedirs(d)
        
        # Copy Executable
        shutil.copy(gui_exe, os.path.join(WINDOWS_DIST_DIR, "veldmap-host-gui.exe"))
        
        # Copy Plugins
        for wasm_file in os.listdir(PLUGINS_DIR):
            if wasm_file.endswith(".wasm"):
                shutil.copy(os.path.join(PLUGINS_DIR, wasm_file), os.path.join(dist_plugins, wasm_file))
        
        # Copy Config
        for config_file in os.listdir(CONFIG_DIR):
            if config_file.endswith(".json"):
                shutil.copy(os.path.join(CONFIG_DIR, config_file), os.path.join(dist_config, config_file))
        
        # Copy .env if exists
        if os.path.exists(".env"):
            print("Deploying .env to build directory...")
            shutil.copy(".env", os.path.join(WINDOWS_DIST_DIR, ".env"))
        
        print(f"Windows x64 build deployed successfully to: {WINDOWS_DIST_DIR}")

def clean():
    """Remove build artifacts."""
    print("Cleaning project...")
    folders_to_remove = ["veldcore/target", "veldgis/target", "veldsdk/rust/target", PLUGINS_DIR]
    for folder in folders_to_remove:
        if os.path.exists(folder):
            print(f"Removing {folder}/")
            shutil.rmtree(folder)
    print("Done.")

def main():
    parser = argparse.ArgumentParser(description="VeldMap Build Script")
    parser.add_argument("command", choices=["build", "clean"], nargs="?", default="build", help="Command to run (default: build)")
    parser.add_argument("--debug", action="store_true", help="Build in debug mode")
    parser.add_argument("--windows", action="store_true", help="Cross-compile for Windows (x86_64)")
    
    args = parser.parse_args()

    if args.command == "clean":
        clean()
    else:
        # 'build' is the default if no command is specified
        build_all(debug=args.debug, windows=args.windows)
        print("\n===============================")
        mode_str = "DEBUG" if args.debug else "RELEASE"
        target_str = " (Windows x64)" if args.windows else ""
        print(f"{mode_str}{target_str} Build complete and successful!")
        print("================================\n")

if __name__ == "__main__":
    main()