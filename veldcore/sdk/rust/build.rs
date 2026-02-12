fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Регистрация фич для подавления ворнингов unexpected_cfgs в зависимых крейтах
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"pdk\", \"wgpu\", \"app\", \"client\"))");

    let proto_files = &[
        "../../proto/core.proto",
        "../../proto/app.proto",
        "../../proto/wgpu.proto"
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(proto_files, &["../../proto/"])?;
    Ok(())
}
