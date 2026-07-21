fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = &["../../../../proto/network.proto"];
    let includes = &["../../../../proto"];
    for p in protos { println!("cargo:rerun-if-changed={}", p); }
    prost_build::Config::new().compile_protos(protos, includes)?;
    Ok(())
}
