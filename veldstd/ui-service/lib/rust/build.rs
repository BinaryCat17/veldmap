fn main() {
    let mut config = prost_build::Config::new();
    config.compile_protos(
        &[
            "../../../proto/ui.proto",
            "../../../../veldcore/proto/core.proto",
            "../../../../veldcore/proto/app.proto"
        ],
        &["../../../proto", "../../../../veldcore/proto"],
    ).unwrap();
}
