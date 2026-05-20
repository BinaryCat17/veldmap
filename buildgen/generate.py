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

    # Template variables
    template_data = {
        "module_name": package_name,
        "version": version,
        "sdk_path": rust_config.get("sdk_path", "../../../../veldcore/sdk/rust"),
        "sdk_features": rust_config.get("sdk_features", []),
        "dependencies": dependencies,
        "rust": {
            "config": rust_config.get("config", "crate::module::Config"),
            "state": rust_config.get("state", "crate::module::State"),
            "init": rust_config.get("init", "crate::module::init"),
        },
        "handlers": handlers
    }

    # Setup Jinja2 environment
    script_dir = os.path.dirname(os.path.abspath(__file__))
    env = Environment(loader=FileSystemLoader(os.path.join(script_dir, "templates")))
    template_rs = env.get_template("lib.rs.j2")
    template_toml = env.get_template("Cargo.toml.j2")

    # Render templates
    rendered_rust = template_rs.render(template_data)
    rendered_toml = template_toml.render(template_data)

    # Save to output directory
    src_dir = os.path.join(args.output_dir, "src")
    os.makedirs(src_dir, exist_ok=True)
    
    lib_rs_path = os.path.join(src_dir, "lib.rs")
    with open(lib_rs_path, 'w') as f:
        f.write(rendered_rust)
        
    cargo_toml_path = os.path.join(args.output_dir, "Cargo.toml")
    with open(cargo_toml_path, 'w') as f:
        f.write(rendered_toml)

    print(f"✅ Generated module at {args.output_dir}")

if __name__ == "__main__":
    main()
