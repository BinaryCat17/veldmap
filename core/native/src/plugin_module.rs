use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use serde::Deserialize;
use extism::{Manifest, Wasm, Plugin, Function};
use crate::dispatcher::{Dispatcher, ServiceLocation};

#[derive(Deserialize)]
struct ServiceEntry {
    location: String,
    path: Option<String>,
    node_id: Option<String>,
}

#[derive(Deserialize)]
struct ServicesManifest {
    services: HashMap<String, ServiceEntry>,
}

pub async fn load_services(dispatcher: Arc<Dispatcher>) -> anyhow::Result<()> {
    load_services_with_functions(dispatcher, vec![]).await
}

pub async fn load_services_with_functions(dispatcher: Arc<Dispatcher>, functions: Vec<Function>) -> anyhow::Result<()> {
    let manifest_path = "config/services.json";
    if !std::path::Path::new(manifest_path).exists() {
        log::warn!("Services manifest not found at {}", manifest_path);
        return Ok(());
    }

    let content = fs::read_to_string(manifest_path)?;
    let manifest: ServicesManifest = serde_json::from_str(&content)?;

    for (name, entry) in manifest.services {
        match entry.location.as_str() {
            "local" => {
                let wasm_path = entry.path.ok_or_else(|| anyhow::anyhow!("Missing path for local service {}", name))?;
                if !std::path::Path::new(&wasm_path).exists() {
                    log::error!("WASM file not found: {}", wasm_path);
                    continue;
                }
                
                let wasm_bytes = fs::read(&wasm_path)?;
                let mut extism_manifest = Manifest::new([Wasm::data(wasm_bytes)]);
                
                // Разрешаем HTTP запросы ко всем хостам (или можно ограничить списком)
                extism_manifest = extism_manifest.with_allowed_host("*");
                
                let service_config_path = format!("config/{}.json", name);
                let service_config = fs::read_to_string(&service_config_path).unwrap_or_else(|_| "{}".to_string());
                extism_manifest.config.insert("config".to_string(), service_config);

                log::info!("Loading local WASM service: {} from {}...", name, wasm_path);
                // Загружаем плагин с переданными хост-функциями
                let plugin = Plugin::new(&extism_manifest, functions.iter().cloned(), true)?;
                log::info!("Plugin {} created successfully.", name);
                
                dispatcher.register_service(name.clone(), ServiceLocation::LocalWasm(Arc::new(Mutex::new(plugin))));
                log::info!("Registered local service: {}", name);
            }
            "remote" => {
                let node_id_str = entry.node_id.ok_or_else(|| anyhow::anyhow!("Missing node_id for remote service {}", name))?;
                let node_id: iroh::NodeId = node_id_str.parse()?;
                dispatcher.register_service(name.clone(), ServiceLocation::RemoteIroh(node_id));
            }
            _ => log::warn!("Unknown service location for {}: {}", name, entry.location),
        }
    }
    Ok(())
}
