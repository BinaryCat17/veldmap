#!/usr/bin/env python3
import subprocess
import os
import sys
import argparse

def main():
    parser = argparse.ArgumentParser(description="VeldMap Run Script")
    parser.add_argument("--debug", action="store_true", help="Run debug build")
    parser.add_argument("--config", default="veldgis/config", help="Path to config directory")
    
    args = parser.parse_args()
    
    profile_flag = [] if args.debug else ["--release"]
    profile_name = "debug" if args.debug else "release"
    
    print(f"Starting VeldMap Native Runtime ({profile_name})...")
    
    # 1. Запускаем Native Host из воркспейса veldcore
    print(f"-> Launching VeldMap Host ({profile_name})...")
    
    env = os.environ.copy()
    env["WGPU_BACKEND"] = "gl"
    env["GALLIUM_DRIVER"] = "d3d12"
    env["EGL_LOG_LEVEL"] = "fatal"
    env["MESA_DEBUG"] = "silent"
    env["LIBGL_DEBUG"] = "quiet"
    
    cmd = [
        "cargo", "run", 
        "--manifest-path", "veldcore/Cargo.toml", 
        "-p", "veldmap-host-gui"
    ] + profile_flag + [
        "--", "--config", args.config
    ]
    
    try:
        subprocess.run(cmd, env=env)
    except KeyboardInterrupt:
        print("\nShutting down.")

if __name__ == "__main__":
    main()
