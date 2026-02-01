fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../../proto/services.proto",
        "../../../proto/ui.proto"
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.compile_protos(proto_files, &["../../../proto/"])?;
    Ok(())
}