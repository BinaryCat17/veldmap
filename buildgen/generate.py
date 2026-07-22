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
    parser.add_argument("--output-dir", help="Absolute path to output directory")
    parser.add_argument("--sdk-stubs",  help="Generate SDK platform stubs to this path and exit")
    args = parser.parse_args()

    script_dir  = os.path.dirname(os.path.abspath(__file__))
    schema_path = os.path.abspath(args.schema)

    # ── Load schema ──────────────────────────────────────────────────────────
    with open(schema_path) as f:
        schema = yaml.safe_load(f)

    # ── SDK platform stubs mode (--sdk-stubs) ────────────────────────────────
    if args.sdk_stubs:
        out_path  = os.path.abspath(args.sdk_stubs)
        svc_name  = schema.get("name")
        pf_inputs = [
            {"name": n, "rust_type": ((d or {}).get("type") or "").split("/")[-1]}
            for n, d in (schema.get("interface", {}).get("inputs", {}) or {}).items()
        ]
        pf_inputs = [i for i in pf_inputs if i["rust_type"]]

        env      = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))
        template = env.get_template("sdk_app.rs.j2")
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "w") as f:
            f.write(template.render({"service_name": svc_name, "inputs": pf_inputs}))
        print(f"✅ Generated SDK platform stubs at {out_path}")
        return

    if not args.output_dir:
        parser.error("--output-dir is required unless --sdk-stubs is given")

    output_dir  = os.path.abspath(args.output_dir)
    schema_dir  = os.path.dirname(schema_path)

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
        for sub_name in (dep_data or {}).get("subs", {}) or {}:
            handlers[f"{dep_name}/{sub_name}"] = f"crate::module::on_sub_{sub_name}"

    # ── Typed emit/call stubs (schema is the source of truth for topics) ─────
    # interface.outputs  → crate::emit::<name>(msg)
    # dependencies.*.calls → crate::calls::<dep_snake>::<name>(msg)
    emits = list(schema.get("interface", {}).get("outputs", {}) or {})

    # interface.inputs → <service>-wrap::inputs::<name>(msg): стабы входных
    # топиков для вызывающих (генерируются в wrap-крейт, см. wrap_lib.rs.j2).
    inputs = [
        {"name": n, "rust_type": ((d or {}).get("type") or "").split("/")[-1]}
        for n, d in (schema.get("interface", {}).get("inputs", {}) or {}).items()
    ]
    inputs = [i for i in inputs if i["rust_type"]]

    dep_calls = []
    for dep_name, dep_data in schema.get("dependencies", {}).items():
        calls = list((dep_data or {}).get("calls", {}) or {})
        if calls:
            dep_calls.append({
                "service": dep_name,
                "snake": dep_name.replace("-", "_"),
                "methods": calls,
            })

    # ── View module (Elm-style view loop) ────────────────────────────────────
    # `view: <dependency>` in schema.yaml names the dependency that renders this
    # module's UI. The generated runner re-renders the module's view after init
    # and after every handled message, shipping the layout to that dependency's
    # wrap crate (which must expose `render::render`). The module must export
    # `pub fn view(&State) -> Element<()>`.
    view_dep = schema.get("view")
    if view_dep and view_dep not in schema.get("dependencies", {}):
        raise SystemExit(f"Schema '{name}': view renderer '{view_dep}' is not declared in dependencies")

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

    # ── Workspace Config ─────────────────────────────────────────────────────
    workspace_path = os.path.join(project_root, "workspace.yaml")
    workspace_data = {}
    if os.path.exists(workspace_path):
        with open(workspace_path) as f:
            workspace_data = yaml.safe_load(f) or {}
    
    sdk_base = workspace_data.get("workspace", {}).get("sdk", "veldcore/sdk")
    sdk_path = os.path.join(workspace_root_rel, sdk_base, "rust").replace("\\", "/")
    
    wrap_sdk_path = os.path.join(os.path.relpath(project_root, os.path.join(output_dir, "wraps", "rust")), sdk_base, "rust").replace("\\", "/")

    # ── Discover dependent protos (from schema.yaml dependencies) ─────────────
    raw_deps    = rust_config.get("dependencies", {})
    proto_paths = []
    dep_protos  = []
    cargo_dependencies = {}
    
    # 1. Add explicitly defined third-party deps
    for dep_name, dep_val in raw_deps.items():
        cargo_dependencies[dep_name] = yaml_dep_to_toml(dep_val)
        
    # 2. Add schema-inferred internal dependencies
    view_wrap_crate = None
    schema_deps = schema.get("dependencies", {})
    for dep_name in schema_deps.keys():
        dep_dir = os.path.normpath(os.path.join(schema_dir, "..", dep_name))
        if os.path.isdir(dep_dir):
            dep_config_path = os.path.join(dep_dir, "config.yaml")
            dep_pkg_name = dep_name
            if os.path.exists(dep_config_path):
                with open(dep_config_path) as df:
                    dep_cfg = yaml.safe_load(df) or {}
                    dep_pkg_name = dep_cfg.get("package", dep_name)
                    
            api_crate_name = f"{dep_pkg_name}-wrap"
            api_crate_snake = api_crate_name.replace("-", "_")
            if dep_name == view_dep:
                view_wrap_crate = api_crate_snake

            # Dependency on the generated wrap crate
            cargo_dependencies[api_crate_name] = f'{{ path = "../../{dep_name}/generated/wraps/rust" }}'
            
            # Extract package name for aliasing
            proto_file = os.path.join(dep_dir, "types.proto")
            if os.path.exists(proto_file):
                pkg = read_proto_package(proto_file)
                if pkg:
                    dep_snake = pkg.split(".")[-1]
                    dep_protos.append({
                        "snake": dep_snake,
                        "api_crate": api_crate_snake,
                    })

    # ── Local proto metadata ─────────────────────────────────────────────────
    local_proto_package = None
    local_proto_path    = None
    if has_local_proto:
        lp = os.path.join(schema_dir, "types.proto")
        rel_to_ws        = os.path.relpath(lp, project_root)
        local_proto_path = os.path.join(workspace_root_rel, rel_to_ws)
        local_proto_package = read_proto_package(lp)



    # ── Template context ─────────────────────────────────────────────────────
    module_name_snake = package_name.replace("-", "_")

    if view_dep and not view_wrap_crate:
        raise SystemExit(f"Schema '{name}': could not resolve wrap crate for view renderer '{view_dep}'")

    template_data = {
        "module_name":        package_name,
        "module_name_snake":  module_name_snake,
        "service_name":       name,
        "version":            version,
        "sdk_path":           sdk_path,
        "dependencies":       cargo_dependencies,
        "rust": {
            "config": "crate::module::Config",
            "state":  "crate::module::State",
            "init":   "crate::module::init",
        },
        "handlers":           handlers,
        "emits":              emits,
        "inputs":             inputs,
        "dep_calls":          dep_calls,
        "view_wrap_crate":    view_wrap_crate,
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
    
    # ── Render API Crate (Wrap) ──────────────────────────────────────────────
    if has_local_proto:
        wrap_dir = os.path.join(output_dir, "wraps", "rust")
        wrap_renders = {
            os.path.join(wrap_dir, "src", "lib.rs"): env.get_template("wrap_lib.rs.j2"),
            os.path.join(wrap_dir, "Cargo.toml"):    env.get_template("wrap_Cargo.toml.j2"),
            os.path.join(wrap_dir, "build.rs"):      env.get_template("wrap_build.rs.j2"),
        }
        renders.update(wrap_renders)
        
        template_data["api_crate_name"] = f"{package_name}-wrap"
        template_data["proto_package"] = local_proto_package
        template_data["has_custom_wrap"] = has_wrap
        template_data["wrap_sdk_path"] = wrap_sdk_path
        template_data["include_proto_dir"] = os.path.join(os.path.relpath(project_root, wrap_dir), "veldcore", "proto").replace("\\", "/")

    for path, template in renders.items():
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(template.render(template_data))

    print(f"✅ Generated module at {output_dir}")


if __name__ == "__main__":
    main()
