use wasmtime::*;
use crate::HostState;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    // 1. veld_host_call (ASYNC)
    linker.func_wrap_async("env", "veld_host_call", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };

            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let req_buf = match data_bytes {
                Some(b) => b,
                None => return Ok(0u64),
            };

            let request = match RpcRequest::decode(&req_buf[..]) {
                Ok(r) => r,
                Err(e) => {
                    log::error!(target: "wasm", "[{}] RpcRequest decode error: {}", caller.data().plugin_name, e);
                    return Ok(0u64);
                }
            };

            let plugin_name = caller.data().plugin_name.clone();
            let dispatcher = caller.data().dispatcher.clone();
            let instance_id = caller.data().instance_id;
            
            log::debug!(target: "wasm", "[{}] Call: {}.{} (ID: {})", plugin_name, request.service, request.method, instance_id);

            let result = if request.service == "system" && request.method == "log" {
                if let Ok(log_req) = crate::core::LogRequest::decode(&request.payload[..]) {
                    use crate::logging::*;
                    let level = match log_req.level() {
                        crate::core::LogLevel::Trace => log::Level::Trace,
                        crate::core::LogLevel::Debug => log::Level::Debug,
                        crate::core::LogLevel::Info => log::Level::Info,
                        crate::core::LogLevel::Warn => log::Level::Warn,
                        crate::core::LogLevel::Error => log::Level::Error,
                    };
                    
                    veld_log(level, log_req.flags | FLAG_WASM, Some(&plugin_name), &log_req.message);
                    Ok(Vec::new())
                } else {
                    dispatcher.call(&request.service, &request.method, request.payload, instance_id).await
                }
            } else {
                dispatcher.call(&request.service, &request.method, request.payload, instance_id).await
            };

            let (payload, error): (Vec<u8>, String) = match result {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            };

            let res_buf = RpcResponse { payload, error, sync: None }.encode_to_vec();
            
            if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                    if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, res_buf.len() as u64).await {
                        let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                        if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr as usize + res_buf.len())) {
                            target.copy_from_slice(&res_buf);
                            return Ok((res_buf.len() as u64) << 32 | res_ptr);
                        }
                    }
                }
            }
            Ok(0u64)
        })
    })?;

    // 2. veld_gpu_write
    linker.func_wrap("env", "veld_gpu_write", |mut caller: Caller<'_, HostState>, id: u64, offset: u64, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        let instance_id = caller.data().instance_id;
        let memory_data = mem.data(&caller);
        if let Some(data_slice) = memory_data.get(ptr as usize..(ptr + len) as usize) {
            let resources = caller.data().resources.clone();
            
            // Проверка ReadOnly и Ownership
            if let Some(entry) = resources.get_resource_entry(id, instance_id) {
                if entry.readonly {
                    log::error!(target: "wasm", "[{}] FAILED gpu_write(id={}): resource is readonly", caller.data().plugin_name, id);
                    return;
                }
            }

            let _ = resources.write_resource(id, offset, data_slice, instance_id);
        }
    })?;

    // 3. veld_gpu_read (ASYNC)
    linker.func_wrap_async("env", "veld_gpu_read", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let resources = caller.data().resources.clone();
            let instance_id = caller.data().instance_id;
            let data_res = tokio::task::block_in_place(|| resources.read_resource(id, offset, len, instance_id));
            
            if let Ok(data) = data_res {
                let mem = match caller.get_export("memory") {
                    Some(Extern::Memory(m)) => m,
                    _ => return Ok(()),
                };
                
                let memory_data = mem.data_mut(&mut caller);
                if let Some(target) = memory_data.get_mut(ptr as usize..(ptr as usize + len as usize)) {
                    let copy_len = data.len().min(len as usize);
                    target[..copy_len].copy_from_slice(&data[..copy_len]);
                    
                    if copy_len >= 4 {
                        log::debug!(target: "wasm", "[{}] gpu_read(id={}) read {} bytes.", caller.data().plugin_name, id, copy_len);
                    }
                } else {
                    log::error!(target: "wasm", "[{}] gpu_read(id={}) FAILED: memory access denied at 0x{:x}", caller.data().plugin_name, id, ptr);
                }
            } else if let Err(e) = data_res {
                log::error!(target: "wasm", "[{}] gpu_read(id={}) FAILED: {}", caller.data().plugin_name, id, e);
            }
            Ok(())
        })
    })?;

    // 13. veld_gpu_create_buffer
    linker.func_wrap("env", "veld_gpu_create_buffer", |mut caller: Caller<'_, HostState>, usage: u32, ptr: u64, len: u64, readonly: u32| -> u64 {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return 0,
        };
        
        let instance_id = caller.data().instance_id;
        let memory_data = mem.data(&caller);
        if let Some(data_slice) = memory_data.get(ptr as usize..(ptr + len) as usize) {
            let resources = caller.data().resources.clone();
            let is_readonly = readonly != 0;
            let id = resources.create_buffer_with_data(data_slice, usage, is_readonly, instance_id);
            log::debug!(target: "wasm", "[{}] Created buffer {} with data (size={}, readonly={})", 
                caller.data().plugin_name, id, len, is_readonly);
            id
        } else {
            0
        }
    })?;

    // 14. veld_gpu_freeze_resource
    linker.func_wrap("env", "veld_gpu_freeze_resource", |caller: Caller<'_, HostState>, id: u64| -> u32 {
        let resources = caller.data().resources.clone();
        let instance_id = caller.data().instance_id;
        if resources.freeze_resource(id, instance_id) {
            log::info!(target: "wasm", "[{}] Frozen resource {}", caller.data().plugin_name, id);
            1
        } else {
            0
        }
    })?;

    // 4. veld_get_info (ASYNC)
    linker.func_wrap_async("env", "veld_get_info", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };
            let key_bytes = match mem.data(&caller).get(ptr as usize..(ptr as usize + len as usize)) {
                Some(b) => b.to_vec(),
                None => return Ok(0u64),
            };
            let key = String::from_utf8_lossy(&key_bytes).into_owned();
            
            let val_opt: Option<String> = caller.data().config.get(&key).map(|v| {
                if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
            });

            if let Some(s) = val_opt {
                let s_bytes = s.as_bytes();
                if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                    if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                        if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, s_bytes.len() as u64).await {
                            let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                            if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr as usize + s_bytes.len())) {
                                target.copy_from_slice(s_bytes);
                                return Ok((s_bytes.len() as u64) << 32 | res_ptr);
                            }
                        }
                    }
                }
            }
            Ok(0u64)
        })
    })?;

    // 5. veld_free
    linker.func_wrap("env", "veld_free", |_: Caller<'_, HostState>, _ptr: u64, _len: u64| {})?;

    // 6. veld_load_u8
    linker.func_wrap("env", "veld_load_u8", |mut caller: Caller<'_, HostState>, ptr: u64| -> u32 {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return 0,
        };
        mem.data(&caller).get(ptr as usize).cloned().unwrap_or(0) as u32
    })?;

    // 7. veld_input_len
    linker.func_wrap("env", "veld_input_len", |caller: Caller<'_, HostState>| -> u64 {
        caller.data().call_context.as_ref()
            .map(|ctx| ctx.0.lock().unwrap().input.len() as u64)
            .unwrap_or(0)
    })?;

    // 8. veld_input_copy
    linker.func_wrap("env", "veld_input_copy", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        let input_data = if let Some(ctx) = &caller.data().call_context {
            ctx.0.lock().unwrap().input.clone()
        } else {
            return;
        };

        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + len as usize)) {
            let copy_len = input_data.len().min(len as usize);
            target[..copy_len].copy_from_slice(&input_data[..copy_len]);
        }
    })?;

    // 9. veld_output_set
    linker.func_wrap("env", "veld_output_set", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        let data = mem.data(&caller).get(ptr as usize..(ptr as usize + len as usize)).map(|s| s.to_vec());
        if let Some(data) = data {
            if let Some(ctx) = &caller.data().call_context {
                let mut inner = ctx.0.lock().unwrap();
                inner.output = data;
            }
        }
    })?;

    // 15. veld_generate_uuid
    linker.func_wrap("env", "veld_generate_uuid", |mut caller: Caller<'_, HostState>, ptr: u64| {
        let uuid = uuid::Uuid::new_v4().to_string();
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + 36)) {
            target.copy_from_slice(uuid.as_bytes());
        }
    })?;

    // 15. veld_task_create
    linker.func_wrap("env", "veld_task_create", |mut caller: Caller<'_, HostState>, ptr: u64| {
        let task_id = uuid::Uuid::new_v4().to_string();
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + 36)) {
            target.copy_from_slice(task_id.as_bytes());
        }

        let dispatcher = caller.data().dispatcher.clone();
        let mut tasks = dispatcher.tasks.lock().unwrap();
        tasks.insert(task_id, crate::dispatcher::TaskState {
            progress: 0.0,
            completed: false,
            error: String::new(),
            abort_handle: None,
            result_handle: None,
            payload: Vec::new(),
        });
    })?;

    // 16. veld_task_update
    linker.func_wrap("env", "veld_task_update", |mut caller: Caller<'_, HostState>, id_ptr: u64, id_len: u64, progress: f32, completed: i32, err_ptr: u64, err_len: u64, py_ptr: u64, py_len: u64| {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        let task_id = String::from_utf8_lossy(mem.data(&caller).get(id_ptr as usize..(id_ptr + id_len) as usize).unwrap_or_default()).to_string();
        let error = String::from_utf8_lossy(mem.data(&caller).get(err_ptr as usize..(err_ptr + err_len) as usize).unwrap_or_default()).to_string();
        let payload = mem.data(&caller).get(py_ptr as usize..(py_ptr + py_len) as usize).unwrap_or_default().to_vec();

        let dispatcher = caller.data().dispatcher.clone();
        let mut tasks = dispatcher.tasks.lock().unwrap();
        if let Some(t) = tasks.get_mut(&task_id) {
            t.progress = progress;
            t.completed = completed != 0;
            if !error.is_empty() { t.error = error; }
            if !payload.is_empty() { t.payload = payload; }
        }
    })?;

    Ok(())
}
