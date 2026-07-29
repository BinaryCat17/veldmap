#!/usr/bin/env python3
"""Dump the `dependencies:` blocks of module schemas as JSON.

Общий минимум между generate.py и build.py без зависимостей, кроме pyyaml:
build.py исполняет этот файл интерпретатором buildgen-venv (системный
Python pyyaml не гарантирует), чтобы топологическая сортировка модулей
читала schema.yaml настоящим YAML-парсером, а не разбором по отступам.

Usage: schema_deps.py <schema.yaml>...  →  stdout: {"<path>": ["dep", ...]}
"""
import json
import sys

import yaml


def read_dependencies(schema_path: str) -> list[str]:
    """Names declared in the schema's top-level `dependencies:` block."""
    with open(schema_path) as f:
        schema = yaml.safe_load(f) or {}
    return list(schema.get("dependencies") or {})


if __name__ == "__main__":
    json.dump({p: read_dependencies(p) for p in sys.argv[1:]}, sys.stdout)
