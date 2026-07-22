fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../proto/core.proto",
        "../../proto/app.proto",
        "../../proto/graphics.proto",
        // Контракты платформенных сервисов (fs, network, tasks)
        "../../proto/fs.proto",
        "../../proto/network.proto",
        "../../proto/tasks.proto",
    ];
    let include_dirs = &["../../proto"];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(proto_files, include_dirs)?;
    Ok(())
}
