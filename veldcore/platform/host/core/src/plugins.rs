use std::fs;
use std::sync::Arc;

use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use crate::dispatcher::Event;
use crate::{HostState, WasmModule, CallContext};



use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_INSTANCE_ID: AtomicU32 = AtomicU32::new(100); // Local plugins start from 100



/// Loads all services from the manifest. Instance ids локальных wasm-сервисов
/// регистрируются в диспетчере (`Dispatcher::instance_of`).
pub async fn load_services(ctx: Arc<crate::setup::HostContext>) -> anyhow::Result<()> {
    let mut config = Config::new();
    config.async_support(true);
    let engine = Engine::new(&config)?;

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

                // Конфиг уезжает в HostState ниже; модуль читает его через
                // ABI-вызов veld_get_config — сервис-посредник не нужен.
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

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

                let plugin_name_clone = name.clone();
                let dispatcher_for_actor = ctx.dispatcher.clone();
                tokio::spawn(async move {
                    while let Some(ev) = rx.recv().await {
                        // handle_event() decodes its input as an EventEnvelope to recover the
                        // "{service}/{method}" topic, so the event is wrapped here.
                        // Конверт кодирует хост, поэтому publisher достоверен:
                        // модуль может авторизовать отправителя по имени.
                        let req = crate::core::EventEnvelope {
                            service: ev.service, method: ev.method, payload: ev.payload,
                            publisher: dispatcher_for_actor.name_of(ev.publisher).unwrap_or_default(),
                        };
                        let call_ctx = CallContext::new(prost::Message::encode_to_vec(&req));
                        wasm_module.store.data_mut().call_context = Some(call_ctx);
                        if let Ok(handle_event) = wasm_module.instance.get_typed_func::<(), i32>(&mut wasm_module.store, "handle_event") {
                            let _ = handle_event.call_async(&mut wasm_module.store, ()).await;
                        }
                    }
                    log::info!("Plugin '{}' actor channel closed, shutting down.", plugin_name_clone);
                });

                ctx.dispatcher.register_instance(name.clone(), instance_id);
                for topic in subs {
                    ctx.dispatcher.register_subscription(topic, tx.clone());
                }
            }
            "remote" => {
                // Распределённость вернётся как мост событий (remote-топики
                // поверх iroh), а не как sync-RPC. См. git-историю: node.rs.
                log::warn!("Remote services are not supported yet, skipping '{}'", name);
            }
            _ => {}
        }
    }
    Ok(())
}
