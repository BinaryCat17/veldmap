fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../interface/core.proto",
        "../../interface/app.proto",
        "../../interface/graphics.proto",
        // Контракты платформенных сервисов (fs, network, tasks)
        "../../interface/modules/fs/fs.proto",
        "../../interface/modules/network/network.proto",
        "../../interface/modules/tasks/tasks.proto",
    ];
    let include_dirs = &["../../interface"];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(proto_files, include_dirs)?;
    Ok(())
}
