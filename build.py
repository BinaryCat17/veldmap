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

MODULES = [
    ("veldmap-data-provider", GIS_MANIFEST, "veldgis/target"),
    ("veldmap-local-storage", GIS_MANIFEST, "veldgis/target"),
    ("veldmap-tile-server", GIS_MANIFEST, "veldgis/target"),
    ("veldmap-render", GIS_MANIFEST, "veldgis/target"),
    ("veldmap-data-browser", GIS_MANIFEST, "veldgis/target"),
    ("veldmap-desktop-client", GIS_MANIFEST, "veldgis/target"),
    ("veld-ui-service", STD_MANIFEST, "veldstd/target"),
]

def run(cmd, cwd=None):
    """Run a shell command and exit on failure."""
    print(f"-> {' '.join(cmd)}")
    res = subprocess.run(cmd, cwd=cwd)
    if res.returncode != 0:
        print(f"\nFATAL: Command failed with exit code {res.returncode}")
        sys.exit(1)

def build_all(debug=False, windows=False):
    """Build WASM modules and Host."""
    profile = "debug" if debug else "release"
    cargo_args = [] if debug else ["--release"]
    
    # 1. Build WASM Modules
    print(f"\n[1/2] Building WASM Modules ({profile})...")
    if not os.path.exists(PLUGINS_DIR):
        os.makedirs(PLUGINS_DIR)

    for module, manifest, target_dir in MODULES:
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