#!/usr/bin/env python3
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

IMPORT = re.compile(r"^  import ([^;]+);$", re.MULTILINE)
EXPORT = re.compile(r"^  export ([^;]+);$", re.MULTILINE)
EXPECTED_EXPORT = "wasi:cli/run@0.3.0"
EXPECTED_TOOL = "wasm-tools 1.262.0"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("artifacts", nargs="+", type=Path)
    parser.add_argument(
        "--config", type=Path, default=Path("runtime/config/component-imports.toml")
    )
    args = parser.parse_args()
    with args.config.open("rb") as source:
        components = tomllib.load(source)["components"]
    tool = os.environ.get("WASM_TOOLS", "wasm-tools")
    version = subprocess.run(
        [tool, "--version"], check=True, capture_output=True, text=True
    ).stdout.strip()
    if version != EXPECTED_TOOL:
        raise RuntimeError(f"expected {EXPECTED_TOOL}, found {version}")

    failed = False
    for artifact in args.artifacts:
        policy = components.get(artifact.stem)
        if policy is None:
            print(f"{artifact}: no component policy", file=sys.stderr)
            failed = True
            continue
        result = subprocess.run(
            [tool, "component", "wit", str(artifact)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        world = result.split("\npackage ", 1)[0]
        imports = set(IMPORT.findall(world))
        exports = set(EXPORT.findall(world))
        expected = set(policy["imports"])
        if imports != expected or exports != {EXPECTED_EXPORT}:
            print(f"{artifact}: interface mismatch", file=sys.stderr)
            print(f"  unexpected imports: {sorted(imports - expected)}", file=sys.stderr)
            print(f"  missing imports: {sorted(expected - imports)}", file=sys.stderr)
            print(f"  exports: {sorted(exports)}", file=sys.stderr)
            failed = True
        else:
            print(f"{artifact}: interfaces OK")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
