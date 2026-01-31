#!/usr/bin/env python3
import os
import shutil
import subprocess
import sys

# Project configuration
PLUGINS_DIR = "plugins"
WASM_TARGET = "wasm32-wasip1"
MODULES = [
    "veldmap-geo-math",
    "veldmap-data-provider",
    "veldmap-local-storage",
    "veldmap-tile-server",
    "veldmap-render",
    "veldmap-core-wasm",
    "veldmap-app-data-browser",
    "veldmap-app-desktop-client"
]

def run(cmd, cwd=None):
    """Run a shell command and exit on failure."""
    print(f"-> {' '.join(cmd)}")
    res = subprocess.run(cmd, cwd=cwd)
    if res.returncode != 0:
        print(f"\nFATAL: Command failed with exit code {res.returncode}")
        sys.exit(1)

def build_all():
    """Build RPC, WASM modules, and Host."""
    # 1. Build RPC Layer
    print("\n[1/3] Building RPC Layer...")
    run(["cargo", "build", "-p", "veldmap-rust-rpc"])

    # 2. Build WASM Modules
    print("\n[2/3] Building WASM Modules...")
    if not os.path.exists(PLUGINS_DIR):
        os.makedirs(PLUGINS_DIR)

    for module in MODULES:
        print(f"\n--- Module: {module} ---")
        run(["cargo", "build", "-p", module, "--target", WASM_TARGET])
        
        # Rust lib names use underscores instead of hyphens
        wasm_file_name = module.replace("-", "_") + ".wasm"
        source_path = os.path.join("target", WASM_TARGET, "debug", wasm_file_name)
        dest_path = os.path.join(PLUGINS_DIR, wasm_file_name)
        
        print(f"Deploying {wasm_file_name} to {PLUGINS_DIR}/")
        shutil.copy(source_path, dest_path)

    # 3. Build Native Host
    print("\n[3/3] Building Native Host...")
    run(["cargo", "build", "-p", "veldmap-native-host"])

def clean():
    """Remove build artifacts."""
    print("Cleaning project...")
    folders_to_remove = ["target", PLUGINS_DIR]
    for folder in folders_to_remove:
        if os.path.exists(folder):
            print(f"Removing {folder}/")
            shutil.rmtree(folder)
    print("Done.")

def main():
    if len(sys.argv) > 1:
        cmd = sys.argv[1]
        if cmd == "clean":
            clean()
        elif cmd == "build":
            build_all()
        else:
            print(f"Unknown command: {cmd}")
            print("Usage: python3 build.py [build|clean]")
            sys.exit(1)
    else:
        build_all()
        print("\n===============================")
        print("Build complete and successful!")
        print("================================\n")

if __name__ == "__main__":
    main()
