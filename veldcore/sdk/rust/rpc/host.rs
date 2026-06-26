#[cfg(feature = "pdk")]
use crate::rpc::core::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

// ── VELD MICROKERNEL ABI ───────────────────────────────────────

#[cfg(feature = "pdk")]
extern "C" {
    fn veld_host_publish(ptr: u64, len: u64);
    fn veld_host_log(level: u64, flags: u64, ptr: u64, len: u64);
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    fn veld_compute_create_resource(ptr: u64, len: u64) -> u64;
    fn veld_compute_execute(ptr: u64, len: u64) -> u64;

    // Arena data access (replaces veld_resource_write/read)
    fn veld_arena_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_arena_read(id: u64, offset: u64, ptr: u64, len: u64);

    // Arena management
    fn veld_arena_alloc(size: u64) -> u64;
    fn veld_arena_alloc_buffer(size: u64, usage: u64, mapped: u64) -> u64;
    fn veld_arena_transfer(region_id: u64, target_module: u64) -> u64;
    fn veld_arena_grant_read(region_id: u64, target_module: u64) -> u64;
    fn veld_arena_revoke(region_id: u64) -> u64;
    fn veld_arena_free(region_id: u64) -> u64;

    // Backward compat (same ABI, old names)
    fn veld_resource_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_resource_read(id: u64, offset: u64, ptr: u64, len: u64);

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

#[cfg(feature = "pdk")]
pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let is_log_call = service == "system" && method == "log";
    if !is_log_call {
        crate::vtrace!(crate::FLAG_SDK, "[SDK-CALL] {}::{} ({} bytes)", service, method, payload.len());
    }
    let request = RpcRequest {
        service: service.to_string(), method: method.to_string(),
        payload, sync: None, instance_id: 0,
    };
    let req_buf = request.encode_to_vec();
    unsafe {
        let res_combined = veld_host_call(req_buf.as_ptr() as u64, req_buf.len() as u64);
        if res_combined == 0 { return Err(anyhow::anyhow!("Host call failed")); }
        let ptr = (res_combined & 0xFFFFFFFF) as *mut u8;
        let len = (res_combined >> 32) as usize;
        let response = RpcResponse::decode(std::slice::from_raw_parts(ptr, len))?;
        veld_free_wasm(ptr as u64, len as u64);
        if !response.error.is_empty() { return Err(anyhow::anyhow!(response.error)); }
        Ok(response.payload)
    }
}

#[cfg(feature = "pdk")]
pub fn compute_create_resource(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let res_combined = veld_compute_create_resource(payload.as_ptr() as u64, payload.len() as u64);
        if res_combined == 0 { return Err(anyhow::anyhow!("Compute create_resource failed")); }
        let ptr = (res_combined & 0xFFFFFFFF) as *mut u8;
        let len = (res_combined >> 32) as usize;
        let response = RpcResponse::decode(std::slice::from_raw_parts(ptr, len))?;
        veld_free_wasm(ptr as u64, len as u64);
        if !response.error.is_empty() { return Err(anyhow::anyhow!(response.error)); }
        Ok(response.payload)
    }
}

#[cfg(feature = "pdk")]
pub fn compute_execute(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let res_combined = veld_compute_execute(payload.as_ptr() as u64, payload.len() as u64);
        if res_combined == 0 { return Err(anyhow::anyhow!("Compute execute failed")); }
        let ptr = (res_combined & 0xFFFFFFFF) as *mut u8;
        let len = (res_combined >> 32) as usize;
        let response = RpcResponse::decode(std::slice::from_raw_parts(ptr, len))?;
        veld_free_wasm(ptr as u64, len as u64);
        if !response.error.is_empty() { return Err(anyhow::anyhow!(response.error)); }
        Ok(response.payload)
    }
}

// ── Arena data access ──────────────────────────────────────────

/// Write data into an arena region
#[cfg(feature = "pdk")]
pub fn arena_write(id: u64, offset: u64, data: &[u8]) {
    unsafe { veld_arena_write(id, offset, data.as_ptr() as u64, data.len() as u64); }
}

/// Read data from an arena region
#[cfg(feature = "pdk")]
pub fn arena_read(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_arena_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

// Backward compat aliases (old names → arena)
#[cfg(feature = "pdk")]
pub fn resource_write(id: u64, offset: u64, data: &[u8]) {
    unsafe { veld_resource_write(id, offset, data.as_ptr() as u64, data.len() as u64); }
}

#[cfg(feature = "pdk")]
pub fn resource_read(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_resource_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

#[cfg(feature = "pdk")]
pub use resource_write as gpu_write_resource;
#[cfg(feature = "pdk")]
pub use resource_read as gpu_read_resource;

// ── Arena management ───────────────────────────────────────────

/// Allocate a CPU data region in the arena
#[cfg(feature = "pdk")]
pub fn arena_alloc(size: u64) -> Option<u64> {
    let id = unsafe { veld_arena_alloc(size) };
    if id == 0 { None } else { Some(id) }
}

/// Allocate a GPU buffer region in the arena
#[cfg(feature = "pdk")]
pub fn arena_alloc_buffer(size: u64, usage: u32, mapped: bool) -> Option<u64> {
    let id = unsafe { veld_arena_alloc_buffer(size, usage as u64, mapped as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Transfer ownership of a region to another module (zero-copy)
#[cfg(feature = "pdk")]
pub fn arena_transfer(region_id: u64, target_module: u32) -> bool {
    unsafe { veld_arena_transfer(region_id, target_module as u64) != 0 }
}

/// Grant read access to another module
#[cfg(feature = "pdk")]
pub fn arena_grant_read(region_id: u64, target_module: u32) -> bool {
    unsafe { veld_arena_grant_read(region_id, target_module as u64) != 0 }
}

/// Revoke all external access to a region
#[cfg(feature = "pdk")]
pub fn arena_revoke(region_id: u64) -> bool {
    unsafe { veld_arena_revoke(region_id) != 0 }
}

/// Free an arena region
#[cfg(feature = "pdk")]
pub fn arena_free(region_id: u64) -> bool {
    unsafe { veld_arena_free(region_id) != 0 }
}

// ── Logging ────────────────────────────────────────────────────

#[cfg(feature = "pdk")]
pub fn log(level: log::Level, flags: u32, message: &str) {
    let level_u64 = match level {
        log::Level::Error => 4u64, log::Level::Warn => 3u64, log::Level::Info => 2u64,
        log::Level::Debug => 1u64, _ => 0u64,
    };
    unsafe { veld_host_log(level_u64, flags as u64, message.as_ptr() as u64, message.len() as u64); }
}

// ── System helpers ─────────────────────────────────────────────

#[cfg(feature = "pdk")]
pub fn get_config(key: &str) -> Option<String> {
    use crate::rpc::core::{GetConfigRequest, GetConfigResponse};
    let req = GetConfigRequest { key: key.to_string() };
    call_service("system", "get_config", req.encode_to_vec())
        .ok().and_then(|res| GetConfigResponse::decode(&res[..]).ok()).map(|r| r.value)
}

// ── Call context ───────────────────────────────────────────────

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

#[cfg(feature = "pdk")]
pub fn publish(topic: &str, payload: Vec<u8>) {
    let parts: Vec<&str> = topic.splitn(2, '/').collect();
    if parts.len() != 2 {
        crate::verror!(crate::FLAG_SDK, "[SDK] Invalid topic: {}", topic);
        return;
    }
    let request = RpcRequest {
        service: parts[0].to_string(), method: parts[1].to_string(),
        payload, sync: None, instance_id: 0,
    };
    let req_buf = request.encode_to_vec();
    unsafe { veld_host_publish(req_buf.as_ptr() as u64, req_buf.len() as u64); }
}
