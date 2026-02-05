fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"pdk\", \"wgpu\", \"app\", \"client\"))");

    let proto_files = &[
        "../../veldcore/proto/core.proto",
        "../../veldcore/proto/app.proto",
        "../../veldcore/proto/wgpu.proto"
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={}", proto);
    }

    let mut config = prost_build::Config::new();
    config.compile_protos(proto_files, &["../../veldcore/proto/"])?;
    Ok(())
}