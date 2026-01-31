fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = prost_build::Config::new();
    config.compile_protos(
        &[
            "../../proto/common.proto", 
            "../../proto/services.proto",
            "../../proto/storage.proto",
            "../../proto/geomath.proto",
            "../../proto/render.proto",
            "../../proto/ui.proto"
        ],
        &["../../proto/"],
    )?;
    Ok(())
}