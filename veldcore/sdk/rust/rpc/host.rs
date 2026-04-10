#[cfg(feature = "pdk")]
use crate::rpc::core::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

// --- VELD MINIMAL MICROKERNEL ABI ---
#[cfg(feature = "pdk")]
#[allow(dead_code)]
extern "C" {
    /// Главная шина сообщений (аналог ioctl). 
    /// Возвращает упакованный u64: (len << 32) | ptr
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    
    /// Прямая запись в ресурс (Zero-copy DMA).
    fn veld_resource_write(id: u64, offset: u64, ptr: u64, len: u64);
    
    /// Прямое чтение из ресурса (Zero-copy DMA).
    fn veld_resource_read(id: u64, offset: u64, ptr: u64, len: u64);
    
    /// Получение длины входных данных вызова.
    fn veld_input_len() -> u64;
    
    /// Копирование входных данных в память WASM.
    fn veld_input_copy(p: u64, n: u64);
    
    /// Установка выходных данных вызова.
    fn veld_output_set(p: u64, n: u64);
}

// --- УПРАВЛЕНИЕ ПАМЯТЬЮ (WASM -> Host) ---

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

// --- СИСТЕМНЫЕ ОБЕРТКИ ---

/// Универсальный вызов любого сервиса Хоста.
#[cfg(feature = "pdk")]
pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    // НЕ логируем вызовы system::log чтобы избежать бесконечной рекурсии
    let is_log_call = service == "system" && method == "log";
    if !is_log_call {
        crate::vtrace!(crate::FLAG_SDK, "[SDK-CALL] {}::{} ({} bytes)", service, method, payload.len());
    }
    
    let request = RpcRequest {
        service: service.to_string(),
        method: method.to_string(),
        payload,
        sync: None,
        instance_id: 0, // Устанавливается хостом автоматически для безопасности
    };
    
    let req_buf = request.encode_to_vec();
    unsafe {
        let res_combined = veld_host_call(req_buf.as_ptr() as u64, req_buf.len() as u64);
        if res_combined == 0 { return Err(anyhow::anyhow!("Host call failed (0 returned)")); }

        let ptr = (res_combined & 0xFFFFFFFF) as *mut u8;
        let len = (res_combined >> 32) as usize;
        
        let res_slice = std::slice::from_raw_parts(ptr, len);
        let response = RpcResponse::decode(res_slice)?;
        
        veld_free_wasm(ptr as u64, len as u64);

        if !response.error.is_empty() { 
            return Err(anyhow::anyhow!(response.error)); 
        }
        
        if !is_log_call {
            crate::vtrace!(crate::FLAG_SDK, "[SDK-CALL] {}::{} OK ({} bytes)", service, method, response.payload.len());
        }
        Ok(response.payload)
    }
}

/// Запись данных в любой ресурс Хоста (Buffer, Texture, Data).
#[cfg(feature = "pdk")]
pub fn resource_write(id: u64, offset: u64, data: &[u8]) {
    unsafe { 
        veld_resource_write(id, offset, data.as_ptr() as u64, data.len() as u64); 
    }
}

/// Чтение данных из любого ресурса Хоста.
#[cfg(feature = "pdk")]
pub fn resource_read(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_resource_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

// --- SERVICE HELPERS (SYSTEM) ---

#[cfg(feature = "pdk")]
pub fn task_create() -> String {
    use crate::rpc::core::{TaskCreateRequest, TaskCreateResponse};
    let req = TaskCreateRequest {};
    call_service("system", "task_create", req.encode_to_vec())
        .and_then(|res| Ok(TaskCreateResponse::decode(&res[..])?.task_id))
        .unwrap_or_default()
}

#[cfg(feature = "pdk")]
pub fn task_update(id: &str, progress: f32, completed: bool, error: &str, payload: &[u8]) {
    use crate::rpc::core::TaskUpdateRequest;
    let req = TaskUpdateRequest {
        task_id: id.to_string(),
        progress,
        completed,
        error: error.to_string(),
        payload: payload.to_vec(),
    };
    let _ = call_service("system", "task_update", req.encode_to_vec());
}

#[cfg(feature = "pdk")]
pub fn get_config(key: &str) -> Option<String> {
    use crate::rpc::core::{GetConfigRequest, GetConfigResponse};
    let req = GetConfigRequest { key: key.to_string() };
    call_service("system", "get_config", req.encode_to_vec())
        .ok()
        .and_then(|res| GetConfigResponse::decode(&res[..]).ok())
        .map(|r| r.value)
}

// --- INTERNAL CONTEXT HELPERS ---

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
    unsafe {
        veld_output_set(data.as_ptr() as u64, data.len() as u64);
    }
}

// Backward compatibility aliases
#[cfg(feature = "pdk")]
pub use resource_write as gpu_write_resource;
#[cfg(feature = "pdk")]
pub use resource_read as gpu_read_resource;
