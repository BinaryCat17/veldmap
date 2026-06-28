use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use serde::Deserialize;
use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::{Dispatcher, ServiceLocation};
use crate::{HostState, WasmModule, CallContext};
use crate::registry::ResourceRegistry;
use crate::memory::MemoryManager;
use crate::graphics::GraphicsDevice;
use crate::window::parse_window_config;

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

use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_INSTANCE_ID: AtomicU32 = AtomicU32::new(100); // Local plugins start from 100

/// Scan plugin configs for window preferences without loading WASM modules
pub fn scan_window_configs(config_dir: &str) -> anyhow::Result<crate::window::PluginWindows> {
    let manifest_path = std::path::Path::new(config_dir).join("services.json");
    if !manifest_path.exists() {
        log::warn!("Manifest not found at {:?}", manifest_path);
        return Ok(crate::window::PluginWindows::new());
    }

    let content = fs::read_to_string(&manifest_path)?;
    let manifest: ServicesManifest = serde_json::from_str(&content)?;
    
    let mut windows = crate::window::PluginWindows::new();

    for (name, entry) in manifest.services {
        if entry.location == "local" {
            let service_config_path = std::path::Path::new(config_dir).join(format!("{}.json", name));
            if let Ok(config_str) = fs::read_to_string(&service_config_path) {
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_str) {
                    if let Some(window_config) = parse_window_config(&config) {
                        log::info!("Plugin '{}' requests window: {}x{} (scale: {})", 
                            name, window_config.width, window_config.height, window_config.ui_scale);
                        windows.add(name, window_config);
                    }
                }
            }
        }
    }
    
    Ok(windows)
}

pub async fn load_services<F>(
    dispatcher: Arc<Dispatcher>,
    registry: Arc<ResourceRegistry>,
    memory: Arc<MemoryManager>,
    graphics: Arc<GraphicsDevice>,
    config_dir: &str,
    mut register_config: F,
    windows: &mut crate::window::PluginWindows,
) -> anyhow::Result<()>
where
    F: FnMut(u32, HashMap<String, serde_json::Value>),
{
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
                
                // Parse window config if present
                let raw_config: serde_json::Value = serde_json::from_str(&service_config_str)?;
                if let Some(window_config) = parse_window_config(&raw_config) {
                    log::info!("Plugin '{}' requests window: {}x{} (scale: {})", 
                        name, window_config.width, window_config.height, window_config.ui_scale);
                    windows.add(name.clone(), window_config);
                }
                
                config_map.insert("config".to_string(), serde_json::Value::String(service_config_str.clone()));
                config_map.insert("plugin_name".to_string(), serde_json::Value::String(name.clone()));
                config_map.insert("surface_format".to_string(), serde_json::Value::Number(graphics.get_surface_format_proto().into()));
                
                let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::SeqCst);
                register_config(instance_id, config_map.clone());
                log::trace!("Loading service '{}' with instance_id {}", name, instance_id);
                
                let mut linker = Linker::new(&engine);
                wasmtime_wasi::p1::add_to_linker_async(&mut linker, |s: &mut HostState| &mut s.wasi)?;
                crate::abi::add_to_linker(&mut linker)?;

                let wasi = WasiCtxBuilder::new()
                    .inherit_stdout()
                    .inherit_stderr()
                    .build_p1();

                let state = HostState {
                    dispatcher: dispatcher.clone(),
                    registry: registry.clone(),
                    memory: memory.clone(),
                    graphics: graphics.clone(),
                    plugin_name: name.clone(),
                    instance_id,
                    config: config_map,
                    call_context: None,
                    wasi,
                    resource_limiter: StoreLimitsBuilder::new().memory_size(1024 * 1024 * 1024).build(),
                };

                let mut store = Store::new(&engine, state);
                linker.define_unknown_imports_as_traps(&module)?;
                let instance = linker.instantiate_async(&mut store, &module).await?;

                // Call init if it exists
                if let Ok(init_func) = instance.get_typed_func::<(), i32>(&mut store, "init") {
                    log::trace!("Calling init for plugin '{}'...", name);
                    
                    let init_input = service_config_str.as_bytes().to_vec();
                    let ctx = CallContext::new(init_input);
                    store.data_mut().call_context = Some(ctx);

                    match init_func.call_async(&mut store, ()).await {
                        Ok(0) => log::info!("Plugin '{}' initialized successfully.", name),
                        Ok(code) => {
                            log::error!("Plugin '{}' failed to initialize with code: {}", name, code);
                            continue;
                        }
                        Err(e) => {
                            log::error!("Error while calling init for '{}': {}", name, e);
                            continue;
                        }
                    }
                    // Reset call context after init
                    store.data_mut().call_context = None;
                }

                let mut wasm_module = WasmModule { store, instance };

                // Extract subscriptions
                let mut subs: Vec<String> = Vec::new();
                if let Ok(get_subs) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "get_subscriptions") {
                    let ctx = CallContext::new(Vec::new());
                    wasm_module.store.data_mut().call_context = Some(ctx.clone());
                    match get_subs.call_async(&mut wasm_module.store, ()).await {
                        Ok(0) => {
                            let out = {
                                let inner = ctx.0.lock().unwrap();
                                inner.output.clone()
                            };
                            if let Ok(topics) = serde_json::from_slice::<Vec<String>>(&out) {
                                subs = topics;
                            }
                        }
                        Ok(code) => log::warn!("Plugin '{}' get_subscriptions returned code: {}", name, code),
                        Err(e) => log::warn!("Plugin '{}' get_subscriptions failed: {}", name, e),
                    }
                    wasm_module.store.data_mut().call_context = None;
                }

                let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::dispatcher::RpcCommand>(100);
                
                let plugin_name_clone = name.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
                    loop {
                        tokio::select! {
                            cmd = rx.recv() => {
                                match cmd {
                                    Some(crate::dispatcher::RpcCommand::Call { method: _, payload, reply, .. }) => {
                                        let ctx = CallContext::new(payload);
                                        wasm_module.store.data_mut().call_context = Some(ctx.clone());
                                        if let Ok(handle_rpc) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "handle_rpc") {
                                            let _ = handle_rpc.call_async(&mut wasm_module.store, ()).await;
                                        }
                                        let out = {
                                            let inner = ctx.0.lock().unwrap();
                                            inner.output.clone()
                                        };
                                        let _ = reply.send(Ok(out));
                                    }
                                    Some(crate::dispatcher::RpcCommand::Notify { payload, .. }) => {
                                        let ctx = CallContext::new(payload);
                                        wasm_module.store.data_mut().call_context = Some(ctx.clone());
                                        if let Ok(handle_rpc) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "handle_rpc") {
                                            let _ = handle_rpc.call_async(&mut wasm_module.store, ()).await;
                                        }
                                    }
                                    None => {
                                        log::info!("Plugin '{}' actor channel closed, shutting down.", plugin_name_clone);
                                        break;
                                    }
                                }
                            }
                            _ = interval.tick() => {
                                if let Ok(poll_tasks) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "poll_tasks") {
                                    let _ = poll_tasks.call_async(&mut wasm_module.store, ()).await;
                                }
                            }
                        }
                    }
                });

                dispatcher.register_service(name.clone(), ServiceLocation::LocalWasm(tx.clone()));
                for topic in subs {
                    dispatcher.register_subscription(topic, ServiceLocation::LocalWasm(tx.clone()));
                }
            }
            "remote" => {
                let node_id_str = entry.node_id.ok_or_else(|| anyhow::anyhow!("Missing node_id for remote service {}", name))?;
                let node_id: iroh::EndpointId = node_id_str.parse()?;
                dispatcher.register_service(name.clone(), ServiceLocation::RemoteIroh(node_id));
            }
            _ => {}
        }
    }
    Ok(())
}
