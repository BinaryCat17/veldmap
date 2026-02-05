#!/usr/bin/env python3
import os
import shutil
import subprocess
import sys
import argparse

# Project configuration
PLUGINS_DIR = "veldgis/plugins"
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

def build_all(debug=False):
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
    print(f"\n[2/2] Building Native Hosts ({profile})...")
    run(["cargo", "build", "--manifest-path", CORE_MANIFEST, "-p", "veldmap-host-gui"] + cargo_args)
    run(["cargo", "build", "--manifest-path", CORE_MANIFEST, "-p", "veldmap-host-cli"] + cargo_args)

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
    parser.add_argument("command", choices=["build", "clean"], nargs="?", default="build")
    parser.add_argument("--debug", action="store_true", help="Build in debug mode")
    
    args = parser.parse_args()

    if args.command == "clean":
        clean()
    else:
        build_all(debug=args.debug)
        print("\n===============================")
        mode_str = "DEBUG" if args.debug else "RELEASE"
        print(f"{mode_str} Build complete and successful!")
        print("================================\n")

if __name__ == "__main__":
    main()