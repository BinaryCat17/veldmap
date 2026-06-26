use wasmtime::*;
use crate::HostState;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    // ── RPC ───────────────────────────────────────────────────

    // veld_host_publish — fire-and-forget
    linker.func_wrap_async("env", "veld_host_publish", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(()),
            };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let req_buf = match data_bytes { Some(b) => b, None => return Ok(()) };
            let request = match RpcRequest::decode(&req_buf[..]) {
                Ok(r) => r,
                Err(e) => { crate::verror!(crate::logging::FLAG_ABI, "[{}] publish decode error: {}", caller.data().plugin_name, e); return Ok(()); }
            };
            let topic = format!("{}/{}", request.service, request.method);
            caller.data().dispatcher.clone().publish(&topic, request.payload);
            Ok(())
        })
    })?;

    // veld_host_log — direct logging
    linker.func_wrap_async("env", "veld_host_log", |mut caller: Caller<'_, HostState>, (level, flags, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            use crate::logging::*;
            use log::Level;
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(()) };
            let msg = mem.data(&caller).get(ptr as usize..(ptr + len) as usize)
                .and_then(|s| std::str::from_utf8(s).ok()).unwrap_or("<invalid>");
            let log_level = match level { 4 => Level::Error, 3 => Level::Warn, 2 => Level::Info, 1 => Level::Debug, _ => Level::Trace };
            veld_log(log_level, flags as u32 | FLAG_WASM, Some(&caller.data().plugin_name.clone()), msg);
            Ok(())
        })
    })?;

    // veld_host_call — main Message Bus
    linker.func_wrap_async("env", "veld_host_call", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let req_buf = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let request = match RpcRequest::decode(&req_buf[..]) {
                Ok(r) => r,
                Err(e) => { crate::verror!(crate::logging::FLAG_ABI, "[{}] call decode error: {}", caller.data().plugin_name, e); return Ok(0u64); }
            };
            let plugin_name = caller.data().plugin_name.clone();
            let dispatcher = caller.data().dispatcher.clone();
            let instance_id = caller.data().instance_id;
            crate::vdebug!(crate::logging::FLAG_ABI, "[ABI] [{}] Call: {}::{} (ID: {})", plugin_name, request.service, request.method, instance_id);
            let result = dispatcher.call(&request.service, &request.method, request.payload, instance_id).await;
            let (payload, error): (Vec<u8>, String) = match result {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            };
            let res_buf = RpcResponse { payload, error, sync: None }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    // ── Arena data access ─────────────────────────────────────

    // veld_arena_write — write data into an arena region (replaces veld_resource_write)
    linker.func_wrap("env", "veld_arena_write", |mut caller: Caller<'_, HostState>, id: u64, offset: u64, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        let instance_id = caller.data().instance_id;
        if let Some(data) = mem.data(&caller).get(ptr as usize..(ptr + len) as usize) {
            let arena = caller.data().resources.arena().clone();
            let _ = arena.write(id, offset, data, instance_id);
        }
    })?;

    // veld_arena_read — read data from an arena region (replaces veld_resource_read)
    linker.func_wrap_async("env", "veld_arena_read", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let arena = caller.data().resources.arena().clone();
            let instance_id = caller.data().instance_id;
            let data = tokio::task::block_in_place(|| arena.read(id, offset, len, instance_id));
            if let Ok(data) = data {
                let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(()) };
                if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + len as usize)) {
                    let copy_len = data.len().min(len as usize);
                    target[..copy_len].copy_from_slice(&data[..copy_len]);
                }
            }
            Ok(())
        })
    })?;

    // ── Arena management ──────────────────────────────────────

    // veld_arena_alloc(size) → region_id
    linker.func_wrap("env", "veld_arena_alloc", |caller: Caller<'_, HostState>, size: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        arena.alloc_cpu(vec![0u8; size as usize], owner_id)
    })?;

    // veld_arena_alloc_buffer(size, usage, mapped) → region_id
    linker.func_wrap("env", "veld_arena_alloc_buffer", |caller: Caller<'_, HostState>, size: u64, usage: u64, mapped: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        arena.alloc_buffer(size, usage as u32, mapped != 0, false, owner_id)
    })?;

    // veld_arena_alloc_texture(width, height, format, usage) → region_id
    linker.func_wrap("env", "veld_arena_alloc_texture", |caller: Caller<'_, HostState>, width: u64, height: u64, format: u64, usage: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        arena.alloc_texture(width as u32, height as u32, format as i32, usage as u32, false, owner_id)
    })?;

    // veld_arena_transfer(region_id, target_module) → bool
    linker.func_wrap("env", "veld_arena_transfer", |caller: Caller<'_, HostState>, region_id: u64, target_module: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        if arena.transfer(region_id, target_module as u32, owner_id) { 1 } else { 0 }
    })?;

    // veld_arena_grant_read(region_id, target_module) → bool
    linker.func_wrap("env", "veld_arena_grant_read", |caller: Caller<'_, HostState>, region_id: u64, target_module: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        if arena.grant_read(region_id, target_module as u32, owner_id) { 1 } else { 0 }
    })?;

    // veld_arena_revoke(region_id) → bool
    linker.func_wrap("env", "veld_arena_revoke", |caller: Caller<'_, HostState>, region_id: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        if arena.revoke_access(region_id, owner_id) { 1 } else { 0 }
    })?;

    // veld_arena_free(region_id) → bool
    linker.func_wrap("env", "veld_arena_free", |caller: Caller<'_, HostState>, region_id: u64| -> u64 {
        let arena = caller.data().resources.arena().clone();
        let owner_id = caller.data().instance_id;
        if arena.free(region_id, owner_id) { 1 } else { 0 }
    })?;

    // ── Call context ──────────────────────────────────────────

    linker.func_wrap("env", "veld_input_len", |caller: Caller<'_, HostState>| -> u64 {
        caller.data().call_context.as_ref()
            .map(|ctx| ctx.0.lock().unwrap().input.len() as u64)
            .unwrap_or(0)
    })?;

    linker.func_wrap("env", "veld_input_copy", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        let input_data = if let Some(ctx) = &caller.data().call_context {
            ctx.0.lock().unwrap().input.clone()
        } else { return };
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + len as usize)) {
            let copy_len = input_data.len().min(len as usize);
            target[..copy_len].copy_from_slice(&input_data[..copy_len]);
        }
    })?;

    linker.func_wrap("env", "veld_output_set", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        let data = mem.data(&caller).get(ptr as usize..(ptr as usize + len as usize)).map(|s| s.to_vec());
        if let Some(data) = data {
            if let Some(ctx) = &caller.data().call_context {
                ctx.0.lock().unwrap().output = data;
            }
        }
    })?;

    // ── Compute (unchanged ABI surface, routes through arena internally) ──

    linker.func_wrap_async("env", "veld_compute_create_resource", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let payload = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let instance_id = caller.data().instance_id;
            let resources = caller.data().resources.clone();
            let service = crate::compute_service::ComputeService::new(resources);
            let result = service.create_resource(payload, instance_id);
            let (res_payload, error) = match result { Ok(p) => (p, String::new()), Err(e) => (Vec::new(), e.to_string()) };
            let res_buf = RpcResponse { payload: res_payload, error, sync: None }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    linker.func_wrap_async("env", "veld_compute_execute", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let payload = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let instance_id = caller.data().instance_id;
            let resources = caller.data().resources.clone();
            let service = crate::compute_service::ComputeService::new(resources);
            let result = service.execute(payload, instance_id);
            let (res_payload, error) = match result { Ok(p) => (p, String::new()), Err(e) => (Vec::new(), e.to_string()) };
            let res_buf = RpcResponse { payload: res_payload, error, sync: None }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    // ── Dummy wbindgen ────────────────────────────────────────

    linker.func_wrap("__wbindgen_placeholder__", "__wbindgen_describe", |_: u32| {})?;
    linker.func_wrap("__wbindgen_placeholder__", "__wbindgen_throw", |_: u32, _: u32| {})?;

    Ok(())
}

/// Helper: write response back to WASM via veld_alloc
async fn write_response_back(caller: &mut Caller<'_, HostState>, res_buf: &[u8]) -> anyhow::Result<u64> {
    if let Some(Extern::Func(alloc_func)) = caller.get_export("veld_alloc") {
        if let Ok(typed_alloc) = alloc_func.typed::<u64, u64>(&caller) {
            if let Ok(res_ptr) = typed_alloc.call_async(&mut *caller, res_buf.len() as u64).await {
                let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
                if let Some(target) = mem.data_mut(&mut *caller).get_mut(res_ptr as usize..(res_ptr as usize + res_buf.len())) {
                    target.copy_from_slice(res_buf);
                    return Ok((res_buf.len() as u64) << 32 | res_ptr);
                }
            }
        }
    }
    Ok(0u64)
}
