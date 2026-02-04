use wasmtime::*;
use crate::HostState;
use crate::services::{RpcRequest, RpcResponse};
use prost::Message;
use std::sync::atomic::Ordering;

pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    // 1. veld_host_call (ASYNC)
    linker.func_wrap_async("env", "veld_host_call", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };

            let data = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let req_buf = match data {
                Some(b) => b,
                None => return Ok(0u64),
            };

            let request = match RpcRequest::decode(&req_buf[..]) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[ABI] RpcRequest decode error: {}", e);
                    return Ok(0u64);
                }
            };

            let dispatcher = caller.data().dispatcher.clone();
            let result = dispatcher.call(&request.service, &request.method, request.payload).await;

            let (payload, error) = match result {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            };

            let res_buf = RpcResponse { payload, error, sync: None }.encode_to_vec();
            
            if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                    if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, res_buf.len() as u64).await {
                        let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                        if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr + res_buf.len() as u64) as usize) {
                            target.copy_from_slice(&res_buf);
                            return Ok((res_buf.len() as u64) << 32 | res_ptr);
                        }
                    }
                }
            }
            Ok(0u64)
        })
    })?;

    // 2. veld_gpu_write (SYNC)
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

    // 3. veld_gpu_read (SYNC)
    linker.func_wrap("env", "veld_gpu_read", |mut caller: Caller<'_, HostState>, id: u64, offset: u64, ptr: u64, len: u64| {
        let resources = caller.data().resources.clone();
        if let Ok(data) = resources.read_resource(id, offset, len) {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return,
            };
            if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr + len) as usize) {
                let copy_len = data.len().min(len as usize);
                target[..copy_len].copy_from_slice(&data[..copy_len]);
            }
        }
    })?;

    // 4. veld_get_info (ASYNC)
    linker.func_wrap_async("env", "veld_get_info", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };
            let key_bytes = match mem.data(&caller).get(ptr as usize..(ptr + len) as usize) {
                Some(b) => b,
                None => return Ok(0u64),
            };
            let key = String::from_utf8_lossy(key_bytes).into_owned();
            
            let val_opt = caller.data().config.get(&key).map(|v| {
                if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
            });

            if let Some(s) = val_opt {
                let s_bytes = s.as_bytes();
                if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                    if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                        if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, s_bytes.len() as u64).await {
                            let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                            if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr + s_bytes.len() as u64) as usize) {
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

    // 5. veld_free (SYNC)
    linker.func_wrap("env", "veld_free", |_: Caller<'_, HostState>, _ptr: u64, _len: u64| {})?;

    // 6. veld_load_u8 (SYNC)
    linker.func_wrap("env", "veld_load_u8", |mut caller: Caller<'_, HostState>, ptr: u64| -> u32 {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return 0,
        };
        mem.data(&caller).get(ptr as usize).cloned().unwrap_or(0) as u32
    })?;

    // 7. veld_input_len (SYNC)
    linker.func_wrap("env", "veld_input_len", |caller: Caller<'_, HostState>| -> u64 {
        caller.data().call_context.as_ref()
            .map(|ctx| ctx.0.lock().unwrap().input.len() as u64)
            .unwrap_or(0)
    })?;

    // 8. veld_input_load_u8 (SYNC)
    linker.func_wrap("env", "veld_input_load_u8", |caller: Caller<'_, HostState>, idx: u64| -> u32 {
        caller.data().call_context.as_ref()
            .and_then(|ctx| ctx.0.lock().unwrap().input.get(idx as usize).cloned())
            .unwrap_or(0) as u32
    })?;

    // 9. veld_output_set (SYNC)
    linker.func_wrap("env", "veld_output_set", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return,
        };
        let data = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
        if let Some(data) = data {
            if let Some(ctx) = &caller.data().call_context {
                let mut inner = ctx.0.lock().unwrap();
                inner.output = data;
            }
        }
    })?;

    // 12. veld_http_request (ASYNC)
    linker.func_wrap_async("env", "veld_http_request", |mut caller: Caller<'_, HostState>, (req_ptr, req_len, body_ptr, body_len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(0u64),
            };

            let req_json = mem.data(&caller).get(req_ptr as usize..(req_ptr + req_len) as usize)
                .map(|b| String::from_utf8_lossy(b).into_owned());
            
            let req_json = match req_json {
                Some(s) => s,
                None => return Ok(0u64),
            };

            let body = if body_len > 0 {
                mem.data(&caller).get(body_ptr as usize..(body_ptr + body_len) as usize).map(|b| b.to_vec())
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
                Err(e) => {
                    log::error!("[ABI] HTTP Request JSON parse error: {}", e);
                    caller.data().last_http_status.store(400, Ordering::SeqCst);
                    return Ok(0u64);
                }
            };

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

            let res = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[ABI] HTTP Execution Error: {}", e);
                    caller.data().last_http_status.store(500, Ordering::SeqCst);
                    return Ok(0u64);
                }
            };

            let status = res.status().as_u16() as u32;
            caller.data().last_http_status.store(status, Ordering::SeqCst);
            
            let body_bytes = res.bytes().await.unwrap_or_default().to_vec();

            if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
                if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
                    if let Ok(res_ptr) = typed_alloc.call_async(&mut caller, body_bytes.len() as u64).await {
                        let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                        if let Some(target) = mem.data_mut(&mut caller).get_mut(res_ptr as usize..(res_ptr + body_bytes.len() as u64) as usize) {
                            target.copy_from_slice(&body_bytes);
                            return Ok((body_bytes.len() as u64) << 32 | res_ptr);
                        }
                    }
                }
            }
            Ok(0u64)
        })
    })?;

    linker.func_wrap("env", "veld_http_status_get", |caller: Caller<'_, HostState>| -> u32 {
        caller.data().last_http_status.load(Ordering::SeqCst)
    })?;

    Ok(())
}