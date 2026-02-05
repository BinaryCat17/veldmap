fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../../proto/common.proto", 
        "../../../proto/dataprovider.proto",
        "../../../proto/storage.proto",
        "../../../proto/geomath.proto",
        "../../../proto/render.proto",
        "../../../proto/tileserver.proto"
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(proto_files, &["../../../proto/"])?;
    Ok(())
}
