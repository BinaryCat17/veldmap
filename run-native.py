#!/usr/bin/env python3
import subprocess
import os
import sys

def main():
    print("Starting VeldMap Native Runtime...")
    
    # 1. Запускаем Native Host из воркспейса veldcore
    # Передаем путь к конфигам внутри veldgis
    print("-> Launching VeldMap Host...")
    try:
        subprocess.run([
            "cargo", "run", 
            "--manifest-path", "veldcore/Cargo.toml", 
            "-p", "veldmap-host-gui", 
            "--release", 
            "--", "--config", "veldgis/config"
        ])
    except KeyboardInterrupt:
        print("\nShutting down.")

if __name__ == "__main__":
    main()