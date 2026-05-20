#!/usr/bin/env python3
import os
import argparse
import yaml
from jinja2 import Environment, FileSystemLoader

def main():
    parser = argparse.ArgumentParser(description="Generate Rust bindings from schema.yaml")
    parser.add_argument("--schema", required=True, help="Path to schema.yaml")
    parser.add_argument("--output-dir", required=True, help="Directory to save generated code")
    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))

    # Load YAML schema
    with open(args.schema, 'r') as f:
        schema = yaml.safe_load(f)

    # Load config.yaml if exists
    config_path = os.path.join(os.path.dirname(args.schema), "config.yaml")
    config_data = {}
    if os.path.exists(config_path):
        with open(config_path, 'r') as f:
            config_data = yaml.safe_load(f)

    name = schema.get("name")
    package_name = config_data.get("package", name)
    version = schema.get("version", "0.1.0")

    rust_config = config_data.get("rust", {})
    dependencies = rust_config.get("dependencies", {})

    # Construct handlers automatically
    handlers = {}
    
    # Inputs
    inputs = schema.get("interface", {}).get("inputs", {})
    for input_name in inputs:
        topic = f"{name}/{input_name}"
        handlers[topic] = f"crate::module::on_input_{input_name}"

    # Subscriptions
    deps = schema.get("dependencies", {})
    for dep_name, dep_data in deps.items():
        subs = dep_data.get("subs", {})
        for sub_name in subs:
            topic = f"{dep_name}/{sub_name}"
            handlers[topic] = f"crate::module::on_sub_{sub_name}"

    # Check for local files
    schema_dir = os.path.dirname(os.path.abspath(args.schema))
    has_local_proto = os.path.exists(os.path.join(schema_dir, "types.proto"))
    has_wrap = os.path.exists(os.path.join(schema_dir, "wraps", "rust", "src", "wrap.rs"))

    # Setup workspace root for proto compilation to avoid shadowing types.proto
    workspace_root_rel = os.path.relpath(os.path.join(script_dir, ".."), args.output_dir)

    # Try to find dependent protos
    proto_paths = []
    include_dirs = [
        workspace_root_rel,
        os.path.join(workspace_root_rel, "veldcore", "proto")
    ]
    dep_protos = [] # Will be list of dicts: {'package': 'veldmap.ui', 'snake': 'ui', 'wrap_path': '...', 'has_wrap': True/False}
    
    # Very simple heuristic: look at rust.dependencies paths
    import re
    for dep_name, dep_val in dependencies.items():
        if isinstance(dep_val, str) and "path" in dep_val:
            # Extract path using regex: { path = "../..." }
            match = re.search(r'path\s*=\s*"([^"]+)"', dep_val)
            if match:
                rel_path = match.group(1)
                abs_dep_path = os.path.normpath(os.path.join(args.output_dir, rel_path))
                
                # Check up to 3 levels up for types.proto
                check_dir = abs_dep_path
                for _ in range(3):
                    if os.path.exists(os.path.join(check_dir, "types.proto")):
                        rel_include = os.path.relpath(check_dir, args.output_dir)
                        proto_file = os.path.join(rel_include, "types.proto")
                        if proto_file not in proto_paths:
                            proto_paths.append(proto_file)
                            
                            # Extract package name
                            package_name_proto = None
                            with open(os.path.join(check_dir, "types.proto"), 'r') as pf:
                                for line in pf:
                                    if line.startswith("package "):
                                        package_name_proto = line.split()[1].strip(";")
                                        break
                                        
                            if package_name_proto:
                                # Check for wrap
                                wrap_path_abs = os.path.join(check_dir, "wraps", "rust", "src", "wrap.rs")
                                has_dep_wrap = os.path.exists(wrap_path_abs)
                                rel_wrap_path = os.path.relpath(wrap_path_abs, args.output_dir) if has_dep_wrap else None
                                
                                dep_protos.append({
                                    "package": package_name_proto,
                                    "snake": package_name_proto.split('.')[-1],
                                    "has_wrap": has_dep_wrap,
                                    "wrap_path": rel_wrap_path
                                })
                        break
                    check_dir = os.path.dirname(check_dir)

    module_name_snake = package_name.replace("-", "_")
    local_proto_package = None
    local_proto_path = None
    if has_local_proto:
        rel_to_ws = os.path.relpath(os.path.join(schema_dir, "types.proto"), os.path.join(script_dir, ".."))
        local_proto_path = os.path.join(workspace_root_rel, rel_to_ws)
        with open(os.path.join(schema_dir, "types.proto"), 'r') as pf:
            for line in pf:
                if line.startswith("package "):
                    local_proto_package = line.split()[1].strip(";")
                    break

    # Filter dependencies for Cargo.toml: only include if they are not path dependencies OR if the path has a Cargo.toml
    cargo_dependencies = {}
    for dep_name, dep_val in dependencies.items():
        if isinstance(dep_val, str) and "path" in dep_val:
            match = re.search(r'path\s*=\s*"([^"]+)"', dep_val)
            if match:
                rel_path = match.group(1)
                abs_dep_path = os.path.normpath(os.path.join(args.output_dir, rel_path))
                if os.path.exists(os.path.join(abs_dep_path, "Cargo.toml")):
                    cargo_dependencies[dep_name] = dep_val
            else:
                cargo_dependencies[dep_name] = dep_val
        else:
            cargo_dependencies[dep_name] = dep_val

    # Template variables
    template_data = {
        "module_name": package_name,
        "module_name_snake": module_name_snake,
        "local_proto_package": local_proto_package,
        "local_proto_path": local_proto_path,
        "version": version,
        "sdk_path": rust_config.get("sdk_path", "../../../../veldcore/sdk/rust"),
        "sdk_features": rust_config.get("sdk_features", []),
        "dependencies": cargo_dependencies,

        "rust": {
            "config": rust_config.get("config", "crate::module::Config"),
            "state": rust_config.get("state", "crate::module::State"),
            "init": rust_config.get("init", "crate::module::init"),
        },
        "handlers": handlers,
        "has_local_proto": has_local_proto,
        "has_wrap": has_wrap,
        "proto_paths": proto_paths,
        "include_dirs": include_dirs,
        "dep_protos": dep_protos,
    }

    # Setup Jinja2 environment
    env = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))
    template_rs = env.get_template("lib.rs.j2")
    template_toml = env.get_template("Cargo.toml.j2")
    template_build = env.get_template("build.rs.j2")

    # Render templates
    rendered_rust = template_rs.render(template_data)
    rendered_toml = template_toml.render(template_data)
    rendered_build = template_build.render(template_data)

    # Save to output directory
    src_dir = os.path.join(args.output_dir, "src")
    os.makedirs(src_dir, exist_ok=True)
    
    lib_rs_path = os.path.join(src_dir, "lib.rs")
    with open(lib_rs_path, 'w') as f:
        f.write(rendered_rust)
        
    cargo_toml_path = os.path.join(args.output_dir, "Cargo.toml")
    with open(cargo_toml_path, 'w') as f:
        f.write(rendered_toml)

    build_rs_path = os.path.join(args.output_dir, "build.rs")
    with open(build_rs_path, 'w') as f:
        f.write(rendered_build)

    print(f"✅ Generated module at {args.output_dir}")

if __name__ == "__main__":
    main()
