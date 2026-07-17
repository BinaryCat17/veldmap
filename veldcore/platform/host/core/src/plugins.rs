use std::fs;
use std::sync::Arc;

use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::ServiceLocation;
use crate::{HostState, WasmModule, CallContext};



use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_INSTANCE_ID: AtomicU32 = AtomicU32::new(100); // Local plugins start from 100



/// Loads all services from the manifest.
/// Returns the instance id assigned to each local WASM service, keyed by name.
pub async fn load_services(
    ctx: Arc<crate::setup::HostContext>,
) -> anyhow::Result<std::collections::HashMap<String, u32>>
{
    let mut config = Config::new();
    config.async_support(true);
    let engine = Engine::new(&config)?;
    let mut instance_ids = std::collections::HashMap::new();

    for (name, entry) in &ctx.config.manifest.services {
        match entry.location.as_str() {
            "local" => {
                let rel_wasm_path = entry.path.as_ref().ok_or_else(|| anyhow::anyhow!("Missing path for local service {}", name))?;
                let wasm_path = ctx.config.project_root.join(rel_wasm_path);
                
                if !wasm_path.exists() {
                    log::error!("WASM file not found: {:?} (resolved from {})", wasm_path, rel_wasm_path);
                    continue;
                }
                
                let wasm_bytes = fs::read(&wasm_path)?;
                let module = Module::from_binary(&engine, &wasm_bytes)?;
                
                let service_config_str = ctx.config.plugin_raw_configs.get(name)
                    .cloned()
                    .unwrap_or_else(|| "{}".to_string());

                let mut config_map = ctx.config.plugin_configs.get(name)
                    .cloned()
                    .unwrap_or_default();
                
                config_map.insert("config".to_string(), serde_json::Value::String(service_config_str.clone()));
                config_map.insert("plugin_name".to_string(), serde_json::Value::String(name.clone()));
                config_map.insert("surface_format".to_string(), serde_json::Value::Number(ctx.graphics.get_surface_format_proto().into()));
                
                let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::SeqCst);
                
                // Register config via RPC
                let reg_req = crate::core::RegisterConfigRequest {
                    key: instance_id,
                    value_json: serde_json::to_string(&config_map).unwrap_or_else(|_| "{}".to_string()),
                };
                if let Err(e) = ctx.dispatcher.call("system", "register_config", prost::Message::encode_to_vec(&reg_req), instance_id).await {
                    log::error!("Failed to register config for '{}': {}", name, e);
                }
                
                log::trace!("Loading service '{}' with instance_id {}", name, instance_id);
                
                let mut linker = Linker::new(&engine);
                wasmtime_wasi::p1::add_to_linker_async(&mut linker, |s: &mut HostState| &mut s.wasi)?;
                crate::abi::add_to_linker(&mut linker)?;

                let wasi = WasiCtxBuilder::new()
                    .inherit_stdout()
                    .inherit_stderr()
                    .build_p1();

                let state = HostState {
                    dispatcher: ctx.dispatcher.clone(),
                    registry: ctx.registry.clone(),
                    memory: ctx.memory.clone(),
                    graphics: ctx.graphics.clone(),
                    plugin_name: name.clone(),
                    instance_id,
                    config: config_map,
                    call_context: None,
                    wasi,
                    resource_limiter: StoreLimitsBuilder::new().memory_size(1024 * 1024 * 1024).build(),
                };

                let mut store = Store::new(&engine, state);
                
                let registry_clone = ctx.registry.clone();
                linker.func_wrap("host", "check_access", move |_caller: Caller<'_, HostState>, id: u64, flags: i32| -> i32 {
                    let access = if flags == 1 { crate::registry::Access::Write } else { crate::registry::Access::Read };
                    if registry_clone.check_access(id, instance_id, access) { 1 } else { 0 }
                })?;

                let mem_clone = ctx.memory.clone();
                linker.func_wrap("host", "read_memory", move |mut caller: Caller<'_, HostState>, id: u64, offset: i64, size: i64, out_ptr: i32| -> i32 {
                    let wasm_mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return -1,
                    };
                    
                    if let Ok(data) = mem_clone.read(id, offset as u64, size as u64) {
                        if wasm_mem.write(&mut caller, out_ptr as usize, &data).is_ok() { 0 } else { -1 }
                    } else { -1 }
                })?;

                let mem_write_clone = ctx.memory.clone();
                linker.func_wrap("host", "write_memory", move |mut caller: Caller<'_, HostState>, id: u64, offset: i64, in_ptr: i32, size: i32| -> i32 {
                    let wasm_mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return -1,
                    };
                    let mem_data = wasm_mem.data(&caller);
                    let data = &mem_data[in_ptr as usize..(in_ptr + size) as usize];
                    if mem_write_clone.write(id, offset as u64, data).is_ok() { 0 } else { -1 }
                })?;

                linker.define_unknown_imports_as_traps(&module)?;
                let instance = linker.instantiate_async(&mut store, &module).await?;

                // Call init if it exists
                if let Ok(init_func) = instance.get_typed_func::<(), i32>(&mut store, "init") {
                    log::trace!("Calling init for plugin '{}'...", name);
                    
                    let init_input = service_config_str.as_bytes().to_vec();
                    let call_ctx = CallContext::new(init_input);
                    store.data_mut().call_context = Some(call_ctx);

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
                    let call_ctx = CallContext::new(Vec::new());
                    wasm_module.store.data_mut().call_context = Some(call_ctx.clone());
                    match get_subs.call_async(&mut wasm_module.store, ()).await {
                        Ok(0) => {
                            let out = {
                                let inner = call_ctx.0.lock().unwrap();
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

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::dispatcher::RpcCommand>();
                
                let plugin_name_clone = name.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
                    loop {
                        tokio::select! {
                            cmd = rx.recv() => {
                                match cmd {
                                    Some(crate::dispatcher::RpcCommand::Call { service_name, method, payload, reply, .. }) => {
                                        // handle_rpc() decodes its input as an RpcRequest to recover the
                                        // "{service}/{method}" topic, so it must be re-wrapped here - the
                                        // dispatcher only carries the bare inner payload internally.
                                        let req = crate::core::RpcRequest {
                                            service: service_name, method, payload, sync: None, instance_id: 0,
                                        };
                                        let call_ctx = CallContext::new(prost::Message::encode_to_vec(&req));
                                        wasm_module.store.data_mut().call_context = Some(call_ctx.clone());
                                        if let Ok(handle_rpc) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "handle_rpc") {
                                            let _ = handle_rpc.call_async(&mut wasm_module.store, ()).await;
                                        }
                                        let out = {
                                            let inner = call_ctx.0.lock().unwrap();
                                            inner.output.clone()
                                        };
                                        let _ = reply.send(Ok(out));
                                    }
                                    Some(crate::dispatcher::RpcCommand::Notify { service_name, method, payload }) => {
                                        let req = crate::core::RpcRequest {
                                            service: service_name, method, payload, sync: None, instance_id: 0,
                                        };
                                        let call_ctx = CallContext::new(prost::Message::encode_to_vec(&req));
                                        wasm_module.store.data_mut().call_context = Some(call_ctx.clone());
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

                ctx.dispatcher.register_service(name.clone(), ServiceLocation::LocalWasm(tx.clone()));
                ctx.dispatcher.register_instance(name.clone(), instance_id);
                for topic in subs {
                    ctx.dispatcher.register_subscription(topic, ServiceLocation::LocalWasm(tx.clone()));
                }
                instance_ids.insert(name.clone(), instance_id);
            }
            "remote" => {
                let node_id_str = entry.node_id.as_ref().ok_or_else(|| anyhow::anyhow!("Missing node_id for remote service {}", name))?;
                let node_id: iroh::EndpointId = node_id_str.parse()?;
                ctx.dispatcher.register_service(name.clone(), ServiceLocation::RemoteIroh(node_id));
            }
            _ => {}
        }
    }
    Ok(instance_ids)
}
