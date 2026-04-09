#!/usr/bin/env python3
import subprocess
import os
import sys
import argparse

def main():
    parser = argparse.ArgumentParser(description="VeldMap Run Script")
    parser.add_argument("--debug", action="store_true", help="Run debug build")
    parser.add_argument("--config", default="veldgis/config", help="Path to config directory")
    parser.add_argument("--vulkan", action="store_true", help="Force Vulkan backend")
    parser.add_argument("--backend", default=None, help="Force specific WGPU backend (vulkan, gl, dx12, etc.)")
    
    # Собираем известные аргументы и все остальные для проброса
    args, extra_args = parser.parse_known_args()
    
    profile_flag = [] if args.debug else ["--release"]
    profile_name = "debug" if args.debug else "release"
    
    print(f"Starting VeldMap Native Runtime ({profile_name})...")
    
    # 1. Запускаем Native Host из воркспейса veldcore
    print(f"-> Launching VeldMap Host ({profile_name})...")
    
    env = os.environ.copy()
    
    # Use Vulkan backend (with dzn support)
    if args.backend:
        env["WGPU_BACKEND"] = args.backend
    elif args.vulkan or args.backend is None:
        env["WGPU_BACKEND"] = "vulkan"
    
    # Enable non-conformant Vulkan drivers (dzn)
    env["WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER"] = "1"
    
    # Logging configuration: 
    # Console: veldmap=info, остальное только error
    # File: всё
    env["RUST_LOG"] = "veldmap=trace,error"
    
    # Suppress GPU driver/MESA logs (всё равно лезут в stderr)
    env["EGL_LOG_LEVEL"] = "fatal"
    env["MESA_DEBUG"] = "silent"
    
    # Logging: в консоль только veldmap info+ и ошибки, в файл - всё
    env["RUST_LOG"] = "veldmap=trace,error"
    
    profile_name = "debug" if args.debug else "release"
    binary_path = f"veldcore/target/{profile_name}/veldmap-host-gui"
    
    cmd = [binary_path, "--config", args.config] + extra_args
    
    print(f"Environment: RUST_LOG={env.get('RUST_LOG', 'not set')}")
    
    # Проверяем, существует ли бинарник
    if not os.path.exists(binary_path):
        print(f"Binary not found: {binary_path}")
        print("Please run build first: python3 build.py")
        return
    
    try:
        # Запускаем процесс напрямую (без cargo)
        # stdout -> консоль, stderr -> host.log
        with open("host.log", "a") as log_file:
            process = subprocess.Popen(
                cmd, 
                env=env,
                stdout=None,  # stdout в консоль
                stderr=subprocess.STDOUT  # stderr тоже в консоль пока
            )
            process.wait()
    except KeyboardInterrupt:
        print("\nShutting down.")

if __name__ == "__main__":
    main()
