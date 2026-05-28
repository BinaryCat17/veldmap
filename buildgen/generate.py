#!/usr/bin/env python3
"""
VeldMap Code Generator

Reads schema.yaml + config.yaml for a module and generates:
  - generated/src/lib.rs       (WASM entry points + dispatch table)
  - generated/Cargo.toml       (standalone workspace, all deps)
  - generated/build.rs         (prost codegen)
  - generated/rust-toolchain.toml
  - generated/.cargo/config.toml
"""
import os
import argparse
import yaml
from jinja2 import Environment, FileSystemLoader


# ── Helpers ───────────────────────────────────────────────────────────────────

def yaml_dep_to_toml(val) -> str:
    """Convert a YAML dependency value to a TOML inline-table or version string.

    Examples:
        "0.14"                              → '"0.14"'
        {version: "1.0", features: [...]}  → '{ version = "1.0", features = [...] }'
        {path: "../../foo/generated"}       → '{ path = "../../foo/generated" }'
    """
    if isinstance(val, str):
        return f'"{val}"'
    if isinstance(val, dict):
        parts = []
        for k, v in val.items():
            if isinstance(v, str):
                parts.append(f'{k} = "{v}"')
            elif isinstance(v, bool):
                parts.append(f'{k} = {"true" if v else "false"}')
            elif isinstance(v, list):
                items = ", ".join(f'"{x}"' for x in v)
                parts.append(f'{k} = [{items}]')
            else:
                parts.append(f'{k} = {v}')
        return "{ " + ", ".join(parts) + " }"
    return str(val)


def dep_path(val) -> str | None:
    """Extract the 'path' field from a dependency value, or None."""
    if isinstance(val, dict):
        return val.get("path")
    return None


def read_proto_package(proto_file: str) -> str | None:
    """Read the 'package' declaration from a .proto file."""
    with open(proto_file) as f:
        for line in f:
            if line.startswith("package "):
                return line.split()[1].strip(";")
    return None


# ── Main ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Generate Rust bindings from schema.yaml")
    parser.add_argument("--schema",     required=True, help="Absolute path to schema.yaml")
    parser.add_argument("--output-dir", required=True, help="Absolute path to output directory")
    args = parser.parse_args()

    script_dir  = os.path.dirname(os.path.abspath(__file__))
    schema_path = os.path.abspath(args.schema)
    output_dir  = os.path.abspath(args.output_dir)
    schema_dir  = os.path.dirname(schema_path)

    # ── Load schema + config ─────────────────────────────────────────────────
    with open(schema_path) as f:
        schema = yaml.safe_load(f)

    config_data = {}
    config_path = os.path.join(schema_dir, "config.yaml")
    if os.path.exists(config_path):
        with open(config_path) as f:
            config_data = yaml.safe_load(f)

    name         = schema.get("name")
    package_name = config_data.get("package", name)
    version      = schema.get("version", "0.1.0")
    rust_config  = config_data.get("rust", {})

    # ── Build handler dispatch table ─────────────────────────────────────────
    handlers = {}

    for input_name in schema.get("interface", {}).get("inputs", {}):
        handlers[f"{name}/{input_name}"] = f"crate::module::on_input_{input_name}"

    for dep_name, dep_data in schema.get("dependencies", {}).items():
        for sub_name in dep_data.get("subs", {}):
            handlers[f"{dep_name}/{sub_name}"] = f"crate::module::on_sub_{sub_name}"

    # ── Detect local proto / wraps ───────────────────────────────────────────
    has_local_proto = os.path.exists(os.path.join(schema_dir, "types.proto"))
    has_wrap        = os.path.exists(os.path.join(schema_dir, "wraps", "rust", "src", "wrap.rs"))

    # Relative path from output_dir to project root (used in build.rs paths)
    project_root    = os.path.normpath(os.path.join(script_dir, ".."))
    workspace_root_rel = os.path.relpath(project_root, output_dir)

    include_dirs = [
        workspace_root_rel,
        os.path.join(workspace_root_rel, "veldcore", "proto"),
    ]

    # ── Discover dependent protos (from path-based dependencies) ─────────────
    raw_deps    = rust_config.get("dependencies", {})
    proto_paths = []
    dep_protos  = []

    for dep_val in raw_deps.values():
        rel = dep_path(dep_val)
        if rel is None:
            continue

        # Resolve path relative to output_dir (where Cargo.toml lives)
        abs_dep = os.path.normpath(os.path.join(output_dir, rel))

        # Walk up to 3 levels looking for types.proto
        check_dir = abs_dep
        for _ in range(3):
            proto_file = os.path.join(check_dir, "types.proto")
            if os.path.exists(proto_file):
                rel_to_ws   = os.path.relpath(proto_file, project_root)
                proto_entry = os.path.join(workspace_root_rel, rel_to_ws)

                if proto_entry not in proto_paths:
                    proto_paths.append(proto_entry)
                    pkg = read_proto_package(proto_file)
                    if pkg:
                        dep_snake = pkg.split(".")[-1]
                        wrap_abs = os.path.join(check_dir, "wraps", "rust", "src", "wrap.rs")
                        has_dep_wrap = os.path.exists(wrap_abs)
                        if has_dep_wrap:
                            # Wrap is included INSIDE pub mod proto { pub mod {snake} { } }
                            # in generated/src/lib.rs (an inline module block).
                            # Rust resolves #[path] relative to the VIRTUAL directory of the
                            # containing inline module, which is src/proto/{snake}/.
                            virtual_dir = os.path.join(output_dir, "src", "proto", dep_snake)
                            wrap_rel = wrap_abs
                        else:
                            wrap_rel = None
                        dep_protos.append({
                            "package":   pkg,
                            "snake":     dep_snake,
                            "has_wrap":  has_dep_wrap,
                            "wrap_path": wrap_rel,
                        })
                break
            check_dir = os.path.dirname(check_dir)

    # ── Local proto metadata ─────────────────────────────────────────────────
    local_proto_package = None
    local_proto_path    = None
    if has_local_proto:
        lp = os.path.join(schema_dir, "types.proto")
        rel_to_ws        = os.path.relpath(lp, project_root)
        local_proto_path = os.path.join(workspace_root_rel, rel_to_ws)
        local_proto_package = read_proto_package(lp)

    # ── Convert dependencies to TOML strings for Cargo.toml template ─────────
    cargo_dependencies = {}
    for dep_name, dep_val in raw_deps.items():
        p = dep_path(dep_val)
        if p is not None:
            # Only include path deps whose target actually exists
            abs_p = os.path.normpath(os.path.join(output_dir, p))
            if not os.path.exists(os.path.join(abs_p, "Cargo.toml")):
                continue
        cargo_dependencies[dep_name] = yaml_dep_to_toml(dep_val)

    # ── Template context ─────────────────────────────────────────────────────
    module_name_snake = package_name.replace("-", "_")

    template_data = {
        "module_name":        package_name,
        "module_name_snake":  module_name_snake,
        "version":            version,
        "sdk_path":           rust_config.get("sdk_path", "../../../veldcore/sdk/rust"),
        "sdk_features":       rust_config.get("sdk_features", []),
        "dependencies":       cargo_dependencies,
        "rust": {
            "config": "crate::module::Config",
            "state":  "crate::module::State",
            "init":   "crate::module::init",
        },
        "handlers":           handlers,
        "has_local_proto":    has_local_proto,
        "local_proto_package": local_proto_package,
        "local_proto_path":   local_proto_path,
        "proto_paths":        proto_paths,
        "include_dirs":       include_dirs,
        "dep_protos":         dep_protos,
    }

    # ── Render templates ─────────────────────────────────────────────────────
    env = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))

    renders = {
        os.path.join(output_dir, "src", "lib.rs"):               env.get_template("lib.rs.j2"),
        os.path.join(output_dir, "Cargo.toml"):                   env.get_template("Cargo.toml.j2"),
        os.path.join(output_dir, "build.rs"):                     env.get_template("build.rs.j2"),
        os.path.join(output_dir, "rust-toolchain.toml"):          env.get_template("rust-toolchain.toml.j2"),
        os.path.join(output_dir, ".cargo", "config.toml"):        env.get_template("cargo-config.toml.j2"),
    }

    for path, template in renders.items():
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(template.render(template_data))

    print(f"✅ Generated module at {output_dir}")


if __name__ == "__main__":
    main()
