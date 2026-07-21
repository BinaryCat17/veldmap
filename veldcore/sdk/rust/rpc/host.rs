use crate::rpc::core::{RpcRequest, RpcResponse};
use prost::Message;

// ── VELD MICROKERNEL ABI ───────────────────────────────────────

extern "C" {
    fn veld_host_publish(ptr: u64, len: u64);
    fn veld_host_log(level: u64, flags: u64, ptr: u64, len: u64);
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    fn veld_get_config(ptr: u64, len: u64) -> u64;
    fn veld_random_bytes(ptr: u64, len: u64);
    fn veld_graphics_create_resource(ptr: u64, len: u64) -> u64;
    fn veld_graphics_execute(ptr: u64, len: u64) -> u64;

    fn veld_memory_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_memory_read(id: u64, offset: u64, ptr: u64, len: u64);

    fn veld_memory_alloc(size: u64) -> u64;
    fn veld_memory_alloc_buffer(size: u64, usage: u64, mapped: u64) -> u64;
    fn veld_memory_alloc_texture(width: u64, height: u64, format: u64, usage: u64) -> u64;
    fn veld_memory_transfer(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_grant_read(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_grant_write(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_revoke(region_id: u64) -> u64;
    fn veld_memory_free(region_id: u64) -> u64;

    fn veld_input_len() -> u64;
    fn veld_input_copy(p: u64, n: u64);
    fn veld_output_set(p: u64, n: u64);
}

// ── WASM MEMORY EXPORTS ────────────────────────────────────────

#[no_mangle]
pub extern "C" fn veld_alloc(size: u64) -> u64 {
    let mut buf: Vec<u8> = Vec::with_capacity(size as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as u64
}

#[no_mangle]
pub unsafe extern "C" fn veld_free_wasm(ptr: u64, size: u64) {
    let _ = Vec::from_raw_parts(ptr as *mut u8, size as usize, size as usize);
}

// ── RPC ────────────────────────────────────────────────────────

/// Забирает сырой буфер хоста: (len << 32 | ptr) → байты, 0 → None.
/// Память под ответ хост выделил через veld_alloc; здесь она освобождается.
unsafe fn take_host_bytes(packed: u64) -> Option<Vec<u8>> {
    if packed == 0 {
        return None;
    }
    let ptr = (packed & 0xFFFF_FFFF) as *mut u8;
    let len = (packed >> 32) as usize;
    let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
    veld_free_wasm(ptr as u64, len as u64);
    Some(bytes)
}

/// Распаковывает ответ хоста: (len << 32 | ptr) → payload из RpcResponse.
unsafe fn take_host_response(packed: u64, what: &str) -> anyhow::Result<Vec<u8>> {
    let buf = take_host_bytes(packed).ok_or_else(|| anyhow::anyhow!("{} failed", what))?;
    let response = RpcResponse::decode(&buf[..])?;
    if !response.error.is_empty() {
        return Err(anyhow::anyhow!(response.error));
    }
    Ok(response.payload)
}

pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    crate::vtrace!(crate::FLAG_SDK, "[SDK-CALL] {}::{} ({} bytes)", service, method, payload.len());
    let request = RpcRequest {
        service: service.to_string(), method: method.to_string(), payload,
        publisher: String::new(),
    };
    let req_buf = request.encode_to_vec();
    unsafe {
        let packed = veld_host_call(req_buf.as_ptr() as u64, req_buf.len() as u64);
        take_host_response(packed, "Host call")
    }
}

pub fn graphics_create_resource(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_graphics_create_resource(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "Graphics create_resource")
    }
}

pub fn graphics_execute(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_graphics_execute(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "Graphics execute")
    }
}

// ── Memory data access ─────────────────────────────────────────

/// Write data into a memory region
pub fn arena_write(id: u64, offset: u64, data: &[u8]) {
    unsafe { veld_memory_write(id, offset, data.as_ptr() as u64, data.len() as u64); }
}

/// Read data from a memory region
pub fn arena_read(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_memory_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

// ── Memory management ──────────────────────────────────────────

/// Allocate a CPU data region in the resource registry
pub fn arena_alloc(size: u64) -> Option<u64> {
    let id = unsafe { veld_memory_alloc(size) };
    if id == 0 { None } else { Some(id) }
}

/// Allocate a GPU buffer region in the resource registry
pub fn arena_alloc_buffer(size: u64, usage: u32, mapped: bool) -> Option<u64> {
    let id = unsafe { veld_memory_alloc_buffer(size, usage as u64, mapped as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Allocate a GPU texture region in the resource registry
pub fn arena_alloc_texture(width: u32, height: u32, format: i32, usage: u32) -> Option<u64> {
    let id = unsafe { veld_memory_alloc_texture(width as u64, height as u64, format as u64, usage as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Transfer ownership of a region to another service (zero-copy)
pub fn arena_transfer(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_transfer(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Grant read access to another service (owner only)
pub fn arena_grant_read(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_grant_read(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Grant write access to another service (owner only).
/// This is how a window owner delegates its render target to a renderer.
pub fn arena_grant_write(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_grant_write(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Revoke all external access to a region
pub fn arena_revoke(region_id: u64) -> bool {
    unsafe { veld_memory_revoke(region_id) != 0 }
}

/// Free a resource region
pub fn arena_free(region_id: u64) -> bool {
    unsafe { veld_memory_free(region_id) != 0 }
}

// ── Logging ────────────────────────────────────────────────────

pub fn log(level: log::Level, flags: u32, message: &str) {
    let level_u64 = match level {
        log::Level::Error => 4u64, log::Level::Warn => 3u64, log::Level::Info => 2u64,
        log::Level::Debug => 1u64, _ => 0u64,
    };
    unsafe { veld_host_log(level_u64, flags as u64, message.as_ptr() as u64, message.len() as u64); }
}

// ── System helpers ─────────────────────────────────────────────

/// Значение из конфига модуля (инжектирован хостом при загрузке).
/// None — ключа нет.
pub fn get_config(key: &str) -> Option<String> {
    unsafe {
        let packed = veld_get_config(key.as_ptr() as u64, key.len() as u64);
        take_host_bytes(packed).and_then(|b| String::from_utf8(b).ok())
    }
}

/// UUID v4 из хостовой энтропии: у wasm нет своего источника случайности.
pub fn generate_id() -> String {
    let mut b = [0u8; 16];
    unsafe { veld_random_bytes(b.as_mut_ptr() as u64, b.len() as u64) };
    b[6] = (b[6] & 0x0f) | 0x40; // версия 4
    b[8] = (b[8] & 0x3f) | 0x80; // вариант RFC 4122
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

// ── Call context ───────────────────────────────────────────────

/// Паблишер обрабатываемого сейчас сообщения (заполняется handle_rpc из
/// конверта, который кодирует хост, — полю можно доверять).
/// Пустая строка — сам хост. Wasm однопоточный, поэтому простого Mutex хватает.
static EVENT_PUBLISHER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn set_event_publisher(name: String) {
    *EVENT_PUBLISHER.lock().unwrap() = name;
}

/// Имя сервиса, опубликовавшего текущее событие ("" — хост).
/// Для авторизации: сравнивайте с ожидаемым отправителем.
pub fn event_publisher() -> String {
    EVENT_PUBLISHER.lock().unwrap().clone()
}

pub fn load_input() -> Vec<u8> {
    unsafe {
        let len = veld_input_len();
        if len == 0 { return Vec::new(); }
        let mut buf = vec![0u8; len as usize];
        veld_input_copy(buf.as_mut_ptr() as u64, len);
        buf
    }
}

pub fn store_output(data: Vec<u8>) {
    unsafe { veld_output_set(data.as_ptr() as u64, data.len() as u64); }
}

// ── Pub/Sub ────────────────────────────────────────────────────

pub fn publish(topic: &str, payload: Vec<u8>) {
    let parts: Vec<&str> = topic.splitn(2, '/').collect();
    if parts.len() != 2 {
        crate::verror!(crate::FLAG_SDK, "[SDK] Invalid topic: {}", topic);
        return;
    }
    // publisher не заполняется: хост игнорирует его во входящих сообщениях
    // и подписывает события сам при доставке.
    let request = RpcRequest {
        service: parts[0].to_string(), method: parts[1].to_string(), payload,
        publisher: String::new(),
    };
    let req_buf = request.encode_to_vec();
    unsafe { veld_host_publish(req_buf.as_ptr() as u64, req_buf.len() as u64); }
}
