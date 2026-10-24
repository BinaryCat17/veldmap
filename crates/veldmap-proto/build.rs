use std::path::PathBuf;

use buffa_codegen::BytesRepr;

const SCHEMAS: [&str; 7] = [
    "common.proto",
    "provider.proto",
    "library.proto",
    "tiler.proto",
    "cache.proto",
    "viewer.proto",
    "globe.proto",
];

fn main() {
    let proto_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let files: Vec<PathBuf> = SCHEMAS.iter().map(|name| proto_dir.join(name)).collect();

    let mut buffa = connectrpc_build::CodeGenConfig::default();
    buffa.generate_views = true;
    buffa.generate_json = true;
    buffa.bytes_fields.push((".".to_owned(), BytesRepr::Bytes));

    connectrpc_build::Config::new()
        .files(&files)
        .includes(&[proto_dir])
        .include_file("_connectrpc.rs")
        .emit_descriptor_set("veldmap_descriptor.bin")
        .buffa_config(buffa)
        .compile()
        .expect("failed to generate VeldMap protobuf bindings");
}
