fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = &["../../../../proto/fs.proto"];
    let includes = &["../../../../proto"];
    for p in protos { println!("cargo:rerun-if-changed={}", p); }
    let mut config = prost_build::Config::new();
    // Разделяемые типы шины (veldmap.core) не перекомпилируются в копию:
    // сгенерированный код ссылается на единственный экземпляр из util.
    config.extern_path(".veldmap.core", "::veldmap_host_util::core");
    config.compile_protos(protos, includes)?;
    Ok(())
}
