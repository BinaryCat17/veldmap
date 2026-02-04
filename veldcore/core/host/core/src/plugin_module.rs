use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use serde::Deserialize;
use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::{Dispatcher, ServiceLocation};
use crate::{HostState, WasmModule, CallContext};
use crate::resources::ResourceManager;

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

pub async fn load_services(
    dispatcher: Arc<Dispatcher>,
    resources: Arc<ResourceManager>,
    config_dir: &str,
) -> anyhow::Result<()> {
    let manifest_path = std::path::Path::new(config_dir).join("services.json");
    if !manifest_path.exists() {
        log::warn!("Manifest not found at {:?}", manifest_path);
        return Ok(());
    }

    let content = fs::read_to_string(&manifest_path)?;
    let manifest: ServicesManifest = serde_json::from_str(&content)?;
    let project_root = std::path::Path::new(config_dir).parent().unwrap_or(std::path::Path::new("."));

    let mut config = Config::new();
    config.async_support(true);
    let engine = Engine::new(&config)?;

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
                let module = Module::from_binary(&engine, &wasm_bytes)?;
                
                let service_config_path = std::path::Path::new(config_dir).join(format!("{}.json", name));
                let mut service_config_str = fs::read_to_string(&service_config_path)
                    .map_err(|e| anyhow::anyhow!("Configuration file not found for service '{}' at {:?}: {}", name, service_config_path, e))?;
                
                for (key, value) in std::env::vars() {
                    let placeholder = format!("${{{}}}", key);
                    service_config_str = service_config_str.replace(&placeholder, &value);
                }

                let mut config_map: HashMap<String, serde_json::Value> = serde_json::from_str(&service_config_str)?;
                config_map.insert("config".to_string(), serde_json::Value::String(service_config_str.clone()));
                
                log::info!("[PLUGIN_MODULE] Loading service '{}'", name);
                
                let mut linker = Linker::new(&engine);
                wasmtime_wasi::p1::add_to_linker_async(&mut linker, |s: &mut HostState| &mut s.wasi)?;
                crate::abi::add_to_linker(&mut linker)?;

                let wasi = WasiCtxBuilder::new()
                    .inherit_stdout()
                    .inherit_stderr()
                    .build_p1();

                let state = HostState {
                    dispatcher: dispatcher.clone(),
                    resources: resources.clone(),
                    plugin_name: name.clone(),
                    config: config_map,
                    call_context: None,
                    wasi,
                    resource_limiter: StoreLimitsBuilder::new().memory_size(1024 * 1024 * 1024).build(),
                    last_http_status: std::sync::atomic::AtomicU32::new(0),
                };

                let mut store = Store::new(&engine, state);
                let instance = linker.instantiate_async(&mut store, &module).await?;

                // Call init if it exists
                if let Ok(init_func) = instance.get_typed_func::<(), i32>(&mut store, "init") {
                    log::info!("[PLUGIN_MODULE] Calling init for plugin '{}'...", name);
                    
                    let init_input = service_config_str.as_bytes().to_vec();
                    let ctx = CallContext::new(init_input);
                    store.data_mut().call_context = Some(ctx);

                    match init_func.call_async(&mut store, ()).await {
                        Ok(0) => log::info!("[PLUGIN_MODULE] Plugin '{}' initialized successfully.", name),
                        Ok(code) => {
                            log::error!("[PLUGIN_MODULE] Plugin '{}' failed to initialize with code: {}", name, code);
                            continue;
                        }
                        Err(e) => {
                            log::error!("[PLUGIN_MODULE] Error while calling init for '{}': {}", name, e);
                            continue;
                        }
                    }
                    // Reset call context after init
                    store.data_mut().call_context = None;
                }

                let wasm_module = WasmModule { store, instance };
                dispatcher.register_service(name.clone(), ServiceLocation::LocalWasm(Arc::new(AsyncMutex::new(wasm_module))));
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
