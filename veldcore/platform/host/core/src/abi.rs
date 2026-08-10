use wasmtime::*;
use crate::HostState;
use crate::core::EventEnvelope;
use prost::Message;

pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    // ── Шина событий ──────────────────────────────────────────

    // veld_host_publish — fire-and-forget
    linker.func_wrap_async("env", "veld_host_publish", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return Ok(()),
            };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let req_buf = match data_bytes { Some(b) => b, None => return Ok(()) };
            let request = match EventEnvelope::decode(&req_buf[..]) {
                Ok(r) => r,
                Err(e) => { log::error!(target: "abi", "[{}] publish decode error: {}", caller.data().plugin_name, e); return Ok(()); }
            };
            let topic = format!("{}/{}", request.service, request.method);
            let publisher = caller.data().instance_id;
            caller.data().dispatcher.clone().publish_from(
                &topic, request.payload, publisher, &request.correlation_id, &request.target);
            Ok(())
        })
    })?;

    // veld_host_log — direct logging
    linker.func_wrap_async("env", "veld_host_log", |mut caller: Caller<'_, HostState>, (level, target_ptr, target_len, ptr, len): (u64, u64, u64, u64, u64)| {
        Box::new(async move {
            use log::Level;
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(()) };
            let read = |p: u64, n: u64| mem.data(&caller).get(p as usize..(p + n) as usize)
                .and_then(|s| std::str::from_utf8(s).ok());
            let msg = read(ptr, len).unwrap_or("<invalid>").to_string();
            let sub = read(target_ptr, target_len).unwrap_or_default().to_string();
            let log_level = match level { 4 => Level::Error, 3 => Level::Warn, 2 => Level::Info, 1 => Level::Debug, _ => Level::Trace };

            // Таргет модуля дополняется его именем: подсистему модуль называет
            // сам ("handlers"), а кто он такой — знает хост, и подделать это
            // через ABI нельзя. Отсюда фильтры вида
            // RUST_LOG=veldmap::ui-service::handlers=trace.
            let plugin = &caller.data().plugin_name;
            let target = if sub.is_empty() {
                format!("veldmap::{}", plugin)
            } else {
                format!("veldmap::{}::{}", plugin, sub)
            };
            log::log!(target: &target, log_level, "{}", msg);
            Ok(())
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
        use rand::Rng;
        let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return };
        if let Some(target) = mem.data_mut(&mut caller).get_mut(ptr as usize..(ptr + len) as usize) {
            rand::rng().fill_bytes(target);
        }
    })?;

    // ── Доступ к байтам ресурса ───────────────────────────────
    // Все три отвечают тегированным ответом (см. tagged_response): у отказа
    // здесь несколько разных причин — нет прав, нет ресурса, носитель только
    // на чтение, не та размерность, — и голый 0 их не различает.

    // veld_resource_write(id, offset, ptr, len) → тегированный ответ.
    linker.func_wrap_async("env", "veld_resource_write", |mut caller: Caller<'_, HostState>, (id, offset, ptr, len): (u64, u64, u64, u64)| {
        Box::new(async move {
            let result = write_bytes(&mut caller, id, Some(offset), ptr, len);
            let buf = tagged_response(result.map(|_| Vec::new()));
            write_response_back(&mut caller, &buf).await
        })
    })?;

    // veld_resource_upload_image(id, ptr, len) → тегированный ответ.
    // Смещения нет намеренно: текстура заливается целиком (см.
    // MemoryManager::upload_image).
    linker.func_wrap_async("env", "veld_resource_upload_image", |mut caller: Caller<'_, HostState>, (id, ptr, len): (u64, u64, u64)| {
        Box::new(async move {
            let result = write_bytes(&mut caller, id, None, ptr, len);
            let buf = tagged_response(result.map(|_| Vec::new()));
            write_response_back(&mut caller, &buf).await
        })
    })?;

    // veld_resource_read(id, offset, size) → тегированный ответ с байтами.
    // Копирует запрошенный диапазон в собственную память вызывающего (не сырой
    // указатель) — та же граница доступа и защита от гонок, что и у записи.
    linker.func_wrap_async("env", "veld_resource_read", |mut caller: Caller<'_, HostState>, (id, offset, size): (u64, u64, u64)| {
        Box::new(async move {
            let instance_id = caller.data().instance_id;
            let registry = caller.data().registry.clone();
            let memory = caller.data().memory.clone();

            let result = if !registry.check_access(id, instance_id, crate::registry::Access::Read) {
                Err(anyhow::anyhow!("read access to resource {} denied", id))
            } else if memory.read_blocks(id) {
                // Диск, сеть и ожидание GPU — на blocking-пул. Гость этого не
                // замечает (вызов для него синхронный), но поток рантайма при
                // медленном носителе остаётся свободным для других плагинов.
                match tokio::task::spawn_blocking(move || memory.read(id, offset, size)).await {
                    Ok(inner) => inner,
                    Err(e) => Err(anyhow::anyhow!("read of resource {} panicked: {}", id, e)),
                }
            } else {
                memory.read(id, offset, size)
            };

            let buf = tagged_response(result);
            write_response_back(&mut caller, &buf).await
        })
    })?;

    // veld_resource_texture_size(id) → (width << 32 | height), 0 — не текстура,
    // не найдена или доступ запрещён. Нужен тому, кто рисует чужую текстуру:
    // вписать её в отведённое место можно только зная её пропорции, а размеры
    // знает только владелец ресурса (обычно — другой сервис).
    linker.func_wrap("env", "veld_resource_texture_size", |caller: Caller<'_, HostState>, id: u64| -> u64 {
        let instance_id = caller.data().instance_id;
        if !caller.data().registry.check_access(id, instance_id, crate::registry::Access::Read) {
            return 0;
        }
        match caller.data().memory.get_texture(id) {
            Some((_, width, height, _)) => (u64::from(width) << 32) | u64::from(height),
            None => 0,
        }
    })?;

    // ── Фоновые задачи ────────────────────────────────────────

    // veld_task_kill(ptr, len) → 1 | 0. ptr/len — сырая строка task_id
    // (correlation_id операции). 1 — операция была жива и снята.
    //
    // Убийство — это изменение состояния хоста, как veld_resource_free,
    // поэтому оно и обращается туда же: прямым
    // синхронным вызовом, а не событием. Права проверяет реестр: убить может
    // заказчик операции или сам хост, и заказчика здесь не назовёшь — он
    // берётся из идентичности вызывающего инстанса.
    //
    // Терминальный ответ за убитого исполнителя публикует диспетчер
    // (Dispatcher::kill), поэтому у заказчика ничего не меняется: конец
    // операции приходит ему одним и тем же топиком в обоих исходах.
    linker.func_wrap("env", "veld_task_kill", |mut caller: Caller<'_, HostState>, ptr: u64, len: u64| -> u64 {
        let Some(id) = read_str(&mut caller, ptr, len) else { return 0 };
        let (dispatcher, requestor) = {
            let data = caller.data();
            (data.dispatcher.clone(), data.instance_id)
        };
        u64::from(dispatcher.kill(&id, requestor))
    })?;

    // ── Memory management ─────────────────────────────────────

    // veld_resource_alloc_buffer(size, usage, mapped) → region_id
    linker.func_wrap("env", "veld_resource_alloc_buffer", |caller: Caller<'_, HostState>, size: u64, usage: u64, mapped: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_buffer(size, usage as u32, mapped != 0, owner_id)
    })?;

    // veld_resource_alloc_cpu(size) → region_id. Обычная CPU-память (Vec<u8>),
    // не GPU-буфер — сосед veld_resource_alloc_buffer, не замена: для случаев,
    // когда wasm-модулю нужно просто переслать байты хосту (например, файл
    // через fs/on_write), а не wgpu usage/mapped-семантика, которая тут
    // не имеет смысла.
    linker.func_wrap("env", "veld_resource_alloc_cpu", |caller: Caller<'_, HostState>, size: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_cpu(vec![0u8; size as usize], owner_id)
    })?;

    // veld_resource_alloc_texture(width, height, format, usage) → region_id
    linker.func_wrap("env", "veld_resource_alloc_texture", |caller: Caller<'_, HostState>, width: u64, height: u64, format: u64, usage: u64| -> u64 {
        let memory = caller.data().memory.clone();
        let owner_id = caller.data().instance_id;
        memory.alloc_texture(width as u32, height as u32, format as i32, usage as u32, owner_id)
    })?;

    // Lease-операции адресуются по имени сервиса — модули нигде не оперируют
    // числовыми instance id. Право менять lease имеет только владелец (или хост).
    // grant_write — делегирование: так владелец окна назначает рендерера текстуры.
    lease_op(linker, "veld_resource_transfer", |lease, target| {
        lease.owner_id = target;
        lease.readers.clear();
        lease.writers.clear();
    })?;
    lease_op(linker, "veld_resource_grant_read", |lease, target| lease.add_reader(target))?;
    lease_op(linker, "veld_resource_grant_write", |lease, target| lease.add_writer(target))?;

    // veld_resource_free(region_id) → bool
    // Освобождает и memory-регионы, и непрозрачные GPU-объекты (view,
    // сэмплеры, bind group'ы): у OwnedResource в SDK один путь освобождения,
    // и на хосте он один — запись о ресурсе едина (реестр), поэтому
    // освобождение атомарно и не перебирает карты носителей.
    linker.func_wrap("env", "veld_resource_free", |caller: Caller<'_, HostState>, region_id: u64| -> u64 {
        let registry = caller.data().registry.clone();
        let owner_id = caller.data().instance_id;
        let can_free = registry.check_access(region_id, owner_id, crate::registry::Access::Write);
        if can_free {
            if registry.unregister(region_id) { 1 } else { 0 }
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

    linker.func_wrap_async("env", "veld_resource_create", |mut caller: Caller<'_, HostState>, (ptr, len): (u64, u64)| {
        Box::new(async move {
            let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return Ok(0u64) };
            let data_bytes = mem.data(&caller).get(ptr as usize..(ptr + len) as usize).map(|s| s.to_vec());
            let payload = match data_bytes { Some(b) => b, None => return Ok(0u64) };
            let instance_id = caller.data().instance_id;
            let graphics = caller.data().graphics.clone();
            let result = graphics.create_resource(payload, instance_id).map(|id| id.to_le_bytes().to_vec());
            let res_buf = tagged_response(result);
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
            let result = graphics.execute(payload, instance_id).map(|()| Vec::new());
            let res_buf = tagged_response(result);
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

/// Читает строку из памяти гостя.
fn read_str(caller: &mut Caller<'_, HostState>, ptr: u64, len: u64) -> Option<String> {
    let mem = match caller.get_export("memory") { Some(Extern::Memory(m)) => m, _ => return None };
    mem.data(&*caller)
        .get(ptr as usize..(ptr + len) as usize)
        .and_then(|s| std::str::from_utf8(s).ok())
        .map(str::to_string)
}

/// Helper: read a service name from WASM memory and resolve its instance id.
fn resolve_service_arg(caller: &mut Caller<'_, HostState>, ptr: u64, len: u64) -> Option<u32> {
    let name = read_str(caller, ptr, len)?;
    let resolved = caller.data().dispatcher.instance_of(&name);
    if resolved.is_none() {
        log::warn!(target: "abi", "[{}] lease grant to unknown service '{}'", caller.data().plugin_name, name);
    }
    resolved
}

/// Кодирует результат синхронного ABI-вызова без protobuf: первый байт —
/// тег (0 = успех, дальше payload; 1 = ошибка, дальше UTF-8 текст).
/// Разбирается на SDK-стороне (sdk/rust/src/abi.rs::take_host_response).
/// Общая часть записи в ресурс: границы гостевой памяти, проверка права и
/// выбор операции. `offset: None` — заливка изображения в текстуру, у неё
/// смещения нет.
fn write_bytes(caller: &mut Caller<'_, HostState>, id: u64, offset: Option<u64>,
               ptr: u64, len: u64) -> anyhow::Result<()> {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return Err(anyhow::anyhow!("guest exports no memory")),
    };
    let instance_id = caller.data().instance_id;
    let registry = caller.data().registry.clone();
    if !registry.check_access(id, instance_id, crate::registry::Access::Write) {
        return Err(anyhow::anyhow!("write access to resource {} denied", id));
    }
    // Cpu — memcpy, GPU — постановка в очередь wgpu: ни то, ни другое поток не
    // держит, поэтому blocking-пул здесь не нужен (в отличие от чтения).
    let memory = caller.data().memory.clone();
    let data = mem.data(&*caller).get(ptr as usize..(ptr + len) as usize)
        .ok_or_else(|| anyhow::anyhow!("source range lies outside guest memory"))?;
    match offset {
        Some(offset) => memory.write(id, offset, data),
        None => memory.upload_image(id, data),
    }
}

fn tagged_response(result: anyhow::Result<Vec<u8>>) -> Vec<u8> {
    match result {
        Ok(payload) => {
            let mut buf = Vec::with_capacity(1 + payload.len());
            buf.push(0);
            buf.extend_from_slice(&payload);
            buf
        }
        Err(e) => {
            let msg = e.to_string();
            let mut buf = Vec::with_capacity(1 + msg.len());
            buf.push(1);
            buf.extend_from_slice(msg.as_bytes());
            buf
        }
    }
}

/// Helper: write response back to WASM via veld_alloc
/// Тип ошибки здесь wasmtime'овский, а не anyhow: это возврат хостовой функции
/// прямо в гостя, и его сигнатуру задаёт `func_wrap_async`. Внутри хоста
/// ошибки остаются anyhow — граница проходит ровно по этой функции.
async fn write_response_back(caller: &mut Caller<'_, HostState>, res_buf: &[u8]) -> wasmtime::Result<u64> {
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
