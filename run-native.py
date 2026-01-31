#!/usr/bin/env python3
import subprocess
import os
import sys

def main():
    print("Starting VeldMap Native Runtime...")
    
    # 1. Запускаем Native Host (который теперь содержит в себе и Core, и Окно)
    print("-> Launching VeldMap Host...")
    try:
        # В этой архитектуре Хост сам загрузит core.wasm и apps/*.wasm
        subprocess.run(["cargo", "run", "-p", "veldmap-native-host", "--release"])
    except KeyboardInterrupt:
        print("\nShutting down.")

if __name__ == "__main__":
    main()