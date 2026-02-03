use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
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

pub async fn load_services(dispatcher: Arc<Dispatcher>, config_dir: &str) -> anyhow::Result<()> {
    load_services_with_functions(dispatcher, vec![], config_dir).await
}

pub async fn load_services_with_functions(dispatcher: Arc<Dispatcher>, functions: Vec<Function>, config_dir: &str) -> anyhow::Result<()> {
    let manifest_path = std::path::Path::new(config_dir).join("services.json");
    if !manifest_path.exists() {
        log::warn!("Manifest not found at {:?}", manifest_path);
        return Ok(());
    }

    let content = fs::read_to_string(&manifest_path)?;
    let manifest: ServicesManifest = serde_json::from_str(&content)?;

    // Корень проекта — это папка, в которой лежит папка config
    let project_root = std::path::Path::new(config_dir).parent().unwrap_or(std::path::Path::new("."));

    for (name, entry) in manifest.services {
        match entry.location.as_str() {
            "local" => {
                let rel_wasm_path = entry.path.ok_or_else(|| anyhow::anyhow!("Missing path for local service {}", name))?;
                let wasm_path = project_root.join(&rel_wasm_path);
                
                if !wasm_path.exists() {
                    log::error!("WASM file not found: {:?} (resolved from {})", wasm_path, rel_wasm_path);
                    continue;
                }
                
                let wasm_bytes = fs::read(&wasm_path)?;
                let mut extism_manifest = Manifest::new([Wasm::data(wasm_bytes)]);
                extism_manifest = extism_manifest.with_allowed_host("*");
                
                // Увеличиваем лимиты памяти (32768 страниц = 2ГБ)
                extism_manifest.memory.max_pages = Some(32768);
                
                let service_config_path = std::path::Path::new(config_dir).join(format!("{}.json", name));
                let mut service_config = fs::read_to_string(&service_config_path)
                    .map_err(|e| anyhow::anyhow!("Configuration file not found for service '{}' at {:?}: {}", name, service_config_path, e))?;
                
                for (key, value) in std::env::vars() {
                    let placeholder = format!("${{{}}}", key);
                    service_config = service_config.replace(&placeholder, &value);
                }

                extism_manifest.config.insert("config".to_string(), service_config);
                extism_manifest.allowed_hosts = Some(vec!["*".to_string()]);
                
                // Предотвращаем падение на локали в WASM
                extism_manifest.config.insert("LANG".to_string(), "en_US.UTF-8".to_string());

                // Загружаем плагин без лишнего вывода в консоль
                let mut plugin = Plugin::new(&extism_manifest, functions.iter().cloned(), true)?;
                
                // Автоматически вызываем init при загрузке
                if plugin.function_exists("init") {
                    log::info!("Calling init for plugin '{}'...", name);
                    if let Err(e) = plugin.call::<(), ()>("init", ()) {
                        log::error!("Failed to initialize plugin '{}': {}", name, e);
                        continue;
                    }
                    log::info!("Plugin '{}' initialized successfully.", name);
                }

                dispatcher.register_service(name.clone(), ServiceLocation::LocalWasm(Arc::new(AsyncMutex::new(plugin))));
            }
            "remote" => {
                let node_id_str = entry.node_id.ok_or_else(|| anyhow::anyhow!("Missing node_id for remote service {}", name))?;
                let node_id: iroh::NodeId = node_id_str.parse()?;
                dispatcher.register_service(name.clone(), ServiceLocation::RemoteIroh(node_id));
            }
            _ => {}
        }
    }
    Ok(())
}