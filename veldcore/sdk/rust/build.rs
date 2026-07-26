use std::path::{Path, PathBuf};

/// Все .proto платформы: инфраструктура в корне interface/ (core, graphics)
/// плюс по одному на сервис в interface/modules/<name>/<name>.proto.
/// Сканирование, а не список: добавление сервиса не требует правки build.rs
/// (то же правило, что у хостовых биндингов — см. buildgen/generate.py).
fn collect_protos(interface: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(interface)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "proto"))
        .collect();

    let modules = interface.join("modules");
    if modules.is_dir() {
        for entry in std::fs::read_dir(&modules)?.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let proto = entry.path().join(format!("{}.proto", name.to_string_lossy()));
            if proto.is_file() {
                files.push(proto);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let interface = Path::new("../../interface");
    let protos = collect_protos(interface)?;

    for proto in &protos {
        println!("cargo:rerun-if-changed={}", proto.display());
    }
    // Новый сервис — это новый каталог, а не изменение существующего файла.
    println!("cargo:rerun-if-changed={}", interface.join("modules").display());

    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    // Один файл с деревом модулей по пакетам — вместо include! на каждый пакет
    // в proto.rs (см. там).
    config.include_file("_protos.rs");
    config.compile_protos(&protos, &[interface])?;
    Ok(())
}
