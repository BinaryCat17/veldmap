fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../proto/core.proto",
        "../../proto/app.proto",
        "../../proto/graphics.proto",
        // Контракты нативных модулей хоста (владельцы — сами модули)
        "../../platform/host/modules/fs/fs.proto",
        "../../platform/host/modules/network/network.proto",
    ];
    let include_dirs = &[
        "../../proto",
        "../../platform/host/modules/fs",
        "../../platform/host/modules/network",
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(proto_files, include_dirs)?;
    Ok(())
}
