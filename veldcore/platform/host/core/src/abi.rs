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
            let publisher = caller.data().instance_id;
            caller.data().dispatcher.clone().publish_from(&topic, request.payload, publisher);
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
            let res_buf = RpcResponse { payload, error }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    // ── System ────────────────────────────────────────────────

    // veld_get_config(key_ptr, key_len) → (len << 32 | ptr), 0 если ключа нет.
    // Конфиг инжектирован загрузчиком плагинов прямо в HostState.
    linker.func_wrap_async("env", "veld_get_config", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let key = match mem.data(&caller).get(ptr as usize..(ptr + len) as usize)
                .and_then(|s| std::str::from_utf8(s).ok()) {
                Some(k) => k.to_string(),
                None => return Ok(0u64),
            };
            let value = match caller.data().config.get(&key) {
                Some(v) => v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()),
                None => return Ok(0u64),
            };
            write_response_back(&mut caller, value.as_bytes()).await
        })
    })?;

    // veld_random_bytes(ptr, len) — хостовая энтропия: у wasm нет своего
    // источника случайности (uuid и пр. собираются на стороне SDK).
    linker.func_wrap("env", "veld_random_bytes", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| {
        use rand::RngCore;
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr + len) as usize) {
            rand::rng().fill_bytes(target);
        }
    })?;

    // ── Memory data access ────────────────────────────────────

    // veld_memory_write — write data into a memory region
    linker.func_wrap("env", "veld_memory_write", |mut caller: Caller<'_, HostState>, id: u64, offset: u64, ptr: u64, len: u64| {
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        let instance_id = caller.data().instance_id;
        if let Some(data) = mem.data(&caller).get(ptr as usize..(ptr + len) as usize) {
            let memory = caller.data().memory.clone();
            let registry = caller.data().registry.clone();
            if registry.check_access(id, instance_id, crate::registry::Access::Write) {
                let _ = memory.write(id, offset, data);
            }
        }
    })?;

    // veld_memory_read — read data from a memory region
    linker.func_wrap_async("env", "veld_memory_read", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let memory = caller.data().memory.clone();
            let registry = caller.data().registry.clone();
            let instance_id = caller.data().instance_id;
            if registry.check_access(id, instance_id, crate::registry::Access::Read) {
                let data = tokio::task::block_in_place(|| memory.read(id, offset, len));
                if let Ok(data) = data {
                    let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(()) };
                    if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr as usize + len as usize)) {
                        let copy_len = data.len().min(len as usize);
                        target[..copy_len].copy_from_slice(&data[..copy_len]);
                    }
                }
            }
            Ok(())
        })
    })?;

    // ── Memory management ─────────────────────────────────────

    // veld_memory_alloc(size) → region_id
    linker.func_wrap("env", "veld_memory_alloc", |caller: Caller<'_, HostState>, size: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_cpu(vec![0u8; size as usize], owner_id)
    })?;

    // veld_memory_alloc_buffer(size, usage, mapped) → region_id
    linker.func_wrap("env", "veld_memory_alloc_buffer", |caller: Caller<'_, HostState>, size: u64, usage: u64, mapped: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_buffer(size, usage as u32, mapped != 0, false, owner_id)
    })?;

    // veld_memory_alloc_texture(width, height, format, usage) → region_id
    linker.func_wrap("env", "veld_memory_alloc_texture", |caller: Caller<'_, HostState>, width: u64, height: u64, format: u64, usage: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_texture(width as u32, height as u32, format as i32, usage as u32, false, owner_id)
    })?;

    // Lease-операции адресуются по имени сервиса — модули нигде не оперируют
    // числовыми instance id. Право менять lease имеет только владелец (или хост).
    // grant_write — делегирование: так владелец окна назначает рендерера текстуры.
    lease_op(linker, "veld_memory_transfer", |lease, target| {
        lease.owner_id = target;
        lease.readers.clear();
        lease.writers.clear();
    })?;
    lease_op(linker, "veld_memory_grant_read", |lease, target| lease.add_reader(target))?;
    lease_op(linker, "veld_memory_grant_write", |lease, target| lease.add_writer(target))?;

    // veld_memory_revoke(region_id) → bool
    linker.func_wrap("env", "veld_memory_revoke", |caller: Caller<'_, HostState>, region_id: u64| -> u64 {
        let registry = caller.data().registry.clone();
        let owner_id = caller.data().instance_id;
        let mut ok = false;
        registry.update_lease(region_id, |lease| {
            if lease.owner_id == owner_id || owner_id == 0 {
                lease.revoke_all();
                ok = true;
            }
        });
        if ok { 1 } else { 0 }
    })?;

    // veld_memory_free(region_id) → bool
    // Освобождает и memory-регионы, и непрозрачные GPU-объекты (view,
    // сэмплеры, bind group'ы): у OwnedResource в SDK один путь освобождения.
    linker.func_wrap("env", "veld_memory_free", |caller: Caller<'_, HostState>, region_id: u64| -> u64 {
        let registry = caller.data().registry.clone();
        let memory = caller.data().memory.clone();
        let graphics = caller.data().graphics.clone();
        let owner_id = caller.data().instance_id;
        let can_free = registry.check_access(region_id, owner_id, crate::registry::Access::Write);
        if can_free {
            if memory.free(region_id) || graphics.free_gpu(region_id) { 1 } else { 0 }
        } else {
            0
        }
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

    // ── Graphics ──────────────────────────────────────────────

    linker.func_wrap_async("env", "veld_graphics_create_resource", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let payload = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let instance_id = caller.data().instance_id;
            let graphics = caller.data().graphics.clone();
            let result = graphics.create_resource(payload, instance_id);
            let (res_payload, error) = match result { Ok(p) => (p, String::new()), Err(e) => (Vec::new(), e.to_string()) };
            let res_buf = RpcResponse { payload: res_payload, error }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    linker.func_wrap_async("env", "veld_graphics_execute", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let payload = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let instance_id = caller.data().instance_id;
            let graphics = caller.data().graphics.clone();
            let result = graphics.execute(payload, instance_id);
            let (res_payload, error) = match result { Ok(p) => (p, String::new()), Err(e) => (Vec::new(), e.to_string()) };
            let res_buf = RpcResponse { payload: res_payload, error }.encode_to_vec();
            write_response_back(&mut caller, &res_buf).await
        })
    })?;

    // ── Dummy wbindgen ────────────────────────────────────────

    linker.func_wrap("__wbindgen_placeholder__", "__wbindgen_describe", |_: u32| {})?;
    linker.func_wrap("__wbindgen_placeholder__", "__wbindgen_throw", |_: u32, _: u32| {})?;

    Ok(())
}

/// Регистрирует ABI-функцию вида `(region_id, name_ptr, name_len) → bool`,
/// меняющую lease ресурса. Проверка владельца — общая для всех операций.
fn lease_op(
    linker: &mut Linker<HostState>,
    name: &'static str,
    apply: impl Fn(&mut crate::registry::Lease, u32) + Send + Sync + Copy + 'static,
) -> anyhow::Result<()> {
    linker.func_wrap("env", name, move |mut caller: Caller<'_, HostState>, region_id: u64, name_ptr: u64, name_len: u64| -> u64 {
        let Some(target) = resolve_service_arg(&mut caller, name_ptr, name_len) else { return 0 };
        let registry = caller.data().registry.clone();
        let owner_id = caller.data().instance_id;
        let mut ok = false;
        registry.update_lease(region_id, |lease| {
            if lease.owner_id == owner_id || owner_id == 0 {
                apply(lease, target);
                ok = true;
            }
        });
        if ok { 1 } else { 0 }
    })?;
    Ok(())
}

/// Helper: read a service name from WASM memory and resolve its instance id.
fn resolve_service_arg(caller: &mut Caller<'_, HostState>, ptr: u64, len: u64) -> Option<u32> {
    let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return None };
    let name = mem.data(&mut *caller).get(ptr as usize..(ptr + len) as usize)
        .and_then(|s| std::str::from_utf8(s).ok())?
        .to_string();
    let resolved = caller.data().dispatcher.instance_of(&name);
    if resolved.is_none() {
        crate::vwarn!(crate::logging::FLAG_ABI, "[{}] lease grant to unknown service '{}'", caller.data().plugin_name, name);
    }
    resolved
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
