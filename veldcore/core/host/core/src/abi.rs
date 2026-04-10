use wasmtime::*;
use crate::HostState;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    // 1. veld_host_call (ASYNC) - The main Message Bus (ioctl)
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
                    crate::verror!(crate::logging::FLAG_ABI, "[{}] RpcRequest decode error: {}", caller.data().plugin_name, e);
                    return Ok(0u64);
                }
            };

            let plugin_name = caller.data().plugin_name.clone();
            let dispatcher = caller.data().dispatcher.clone();
            let instance_id = caller.data().instance_id;
            
            crate::vdebug!(crate::logging::FLAG_ABI, "[ABI] [{}] Call: {}::{} (ID: {})", plugin_name, request.service, request.method, instance_id);

            // Special handling for log to avoid circular dependencies and for performance
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
                Ok(p) => {
                    crate::vtrace!(crate::logging::FLAG_ABI, "[ABI] [{}] Call OK: {}::{} ({} bytes)", plugin_name, request.service, request.method, p.len());
                    (p, String::new())
                },
                Err(e) => {
                    crate::verror!(crate::logging::FLAG_ABI, "[ABI] [{}] Call ERR: {}::{} - {}", plugin_name, request.service, request.method, e);
                    (Vec::new(), e.to_string())
                },
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

    // 2. veld_resource_write (Zero-copy DMA-like write)
    linker.func_wrap("env", "veld_resource_write", |mut caller: Caller<'_, HostState>, id: u64, offset: u64, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        
        let instance_id = caller.data().instance_id;
        let memory_data = mem.data(&caller);
        if let Some(data_slice) = memory_data.get(ptr as usize..(ptr + len) as usize) {
            let resources = caller.data().resources.clone();
            let _ = resources.write_resource(id, offset, data_slice, instance_id);
        }
    })?;

    // 3. veld_resource_read (ASYNC) (Zero-copy DMA-like read)
    linker.func_wrap_async("env", "veld_resource_read", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
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
                }
            }
            Ok(())
        })
    })?;

    // 4. veld_input_len
    linker.func_wrap("env", "veld_input_len", |caller: Caller<'_, HostState>| -> u64 {
        caller.data().call_context.as_ref()
            .map(|ctx| ctx.0.lock().unwrap().input.len() as u64)
            .unwrap_or(0)
    })?;

    // 5. veld_input_copy
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

    // 6. veld_output_set
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

    Ok(())
}
