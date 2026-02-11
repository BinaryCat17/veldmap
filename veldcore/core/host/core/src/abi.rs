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
            
            log::debug!(target: "wasm", "[{}] Call: {}.{}", plugin_name, request.service, request.method);

            let result = if request.service == "system" && request.method == "log" {
                if let Ok(log_req) = crate::core::LogRequest::decode(&request.payload[..]) {
                    log::info!(target: "wasm", "[{}] {}", plugin_name, log_req.message);
                    Ok(Vec::new())
                } else {
                    dispatcher.call(&request.service, &request.method, request.payload).await
                }
            } else {
                dispatcher.call(&request.service, &request.method, request.payload).await
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
        let data = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
        if let Some(data) = data {
            let resources = caller.data().resources.clone();
            let _ = resources.write_resource(id, offset, &data);
        }
    })?;

    // 3. veld_gpu_read (ASYNC)
    linker.func_wrap_async("env", "veld_gpu_read", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let resources = caller.data().resources.clone();
            let data_res = tokio::task::block_in_place(|| resources.read_resource(id, offset, len));
            
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

    // 12. veld_http_request (ASYNC)
    linker.func_wrap_async("env", "veld_http_request", |mut caller: Caller<'_, HostState>, (req_ptr, req_len, body_ptr, body_len, status_ptr): (u64, u64, u64, u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };

            let req_json_bytes = mem.data(&caller).get(req_ptr as usize..(req_ptr as usize + req_len as usize))
                .map(|b| b.to_vec());
            
            let req_json = match req_json_bytes {
                Some(b) => String::from_utf8_lossy(&b).into_owned(),
                None => return Ok(0u64),
            };

            let body = if body_len > 0 {
                mem.data(&caller).get(body_ptr as usize..(body_ptr as usize + body_len as usize)).map(|b| b.to_vec())
            } else {
                None
            };

            #[derive(serde::Deserialize)]
            struct Req {
                url: String,
                method: Option<String>,
                headers: std::collections::HashMap<String, String>,
            }

            let req_data: Req = match serde_json::from_str(&req_json) {
                Ok(r) => r,
                Err(_) => return Ok(0u64),
            };

            let plugin_name = caller.data().plugin_name.clone();
            log::info!(target: "wasm", "[{}] HTTP {} {}", plugin_name, req_data.method.as_deref().unwrap_or("GET"), req_data.url);

            let client = reqwest::Client::new();
            let method = match req_data.method.as_deref().unwrap_or("GET") {
                "POST" => reqwest::Method::POST,
                "PUT" => reqwest::Method::PUT,
                "DELETE" => reqwest::Method::DELETE,
                _ => reqwest::Method::GET,
            };

            let mut builder = client.request(method, &req_data.url);
            for (k, v) in req_data.headers { builder = builder.header(k, v); }
            if let Some(b) = body { builder = builder.body(b); }

            let res_result = builder.send().await;
            let (status, body_bytes) = match res_result {
                Ok(r) => {
                    let s = r.status().as_u16() as u32;
                    let b = r.bytes().await.unwrap_or_default().to_vec();
                    (s, b)
                }
                Err(e) => {
                    log::error!(target: "wasm", "[{}] HTTP Error: {}", plugin_name, e);
                    (500, Vec::new())
                }
            };

            let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
            if let Some(status_target) = mem.data_mut(&mut caller).get_mut(status_ptr as usize..(status_ptr as usize + 4)) {
                status_target.copy_from_slice(&status.to_le_bytes());
            }

            if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                    if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, body_bytes.len() as u64).await {
                        let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                        if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr as usize + body_bytes.len())) {
                            target.copy_from_slice(&body_bytes);
                            return Ok((body_bytes.len() as u64) << 32 | res_ptr);
                        }
                    }
                }
            }
            Ok(0u64)
        })
    })?;

    Ok(())
}
