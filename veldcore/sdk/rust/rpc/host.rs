#[cfg(feature = "pdk")]
use crate::rpc::core::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

// --- VELD MINIMAL MICROKERNEL ABI ---
#[cfg(feature = "pdk")]
#[allow(dead_code)]
extern "C" {
    // Message Bus (ioctl)
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    
    // Zero-copy DMA-like access
    fn veld_resource_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_resource_read(id: u64, offset: u64, ptr: u64, len: u64);
    
    // Call Context exchange
    fn veld_input_len() -> u64;
    fn veld_input_copy(p: u64, n: u64);
    fn veld_output_set(p: u64, n: u64);
}

#[cfg(feature = "pdk")]
pub fn task_create() -> String {
    use crate::rpc::core::{TaskCreateRequest, TaskCreateResponse};
    let req = TaskCreateRequest {};
    match call_service("system", "task_create", req.encode_to_vec()) {
        Ok(res) => {
            match TaskCreateResponse::decode(&res[..]) {
                Ok(r) => r.task_id,
                Err(_) => String::new(),
            }
        }
        Err(_) => String::new(),
    }
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

#[cfg(feature = "pdk")]
pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let request = RpcRequest {
        service: service.to_string(),
        method: method.to_string(),
        payload,
        sync: None,
        instance_id: 0,
    };
    
    let req_buf = request.encode_to_vec();
    unsafe {
        let res_ptr = veld_host_call(req_buf.as_ptr() as u64, req_buf.len() as u64);
        if res_ptr == 0 { return Err(anyhow::anyhow!("Veld System Call failed")); }

        let ptr = (res_ptr & 0xFFFFFFFF) as *mut u8;
        let len = (res_ptr >> 32) as usize;
        
        let res_buf = std::slice::from_raw_parts(ptr, len);
        let response = RpcResponse::decode(res_buf)?;
        
        veld_free_wasm(ptr as u64, len as u64);

        if !response.error.is_empty() { return Err(anyhow::anyhow!(response.error)); }
        Ok(response.payload)
    }
}

#[cfg(feature = "pdk")]
pub fn gpu_write_resource(id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
    unsafe { 
        veld_resource_write(id, offset, data.as_ptr() as u64, data.len() as u64); 
    }
    Ok(())
}

#[cfg(feature = "pdk")]
pub fn gpu_read_resource(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_resource_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

#[cfg(feature = "pdk")]
pub fn compute_create_buffer(usage: u32, size: u64, readonly: bool, mapped: bool) -> anyhow::Result<u64> {
    use crate::rpc::compute::{ComputeResourceRequest, ComputeResourceResponse, CreateBuffer, compute_resource_request::Command};
    let req = ComputeResourceRequest {
        command: Some(Command::CreateBuffer(CreateBuffer {
            size,
            usage,
            mapped_at_creation: mapped,
            readonly,
        })),
        instance_id: 0,
    };
    let res_bytes = call_service("compute", "create_resource", req.encode_to_vec())?;
    let res = ComputeResourceResponse::decode(&res_bytes[..])?;
    if let Some(h) = res.handle {
        Ok(h.id)
    } else {
        Err(anyhow::anyhow!("Buffer creation failed: {}", res.error))
    }
}

#[cfg(feature = "pdk")]
pub fn get_config(key: &str) -> Option<String> {
    use crate::rpc::core::{GetConfigRequest, GetConfigResponse};
    let req = GetConfigRequest { key: key.to_string() };
    match call_service("system", "get_config", req.encode_to_vec()) {
        Ok(res_bytes) => {
            match GetConfigResponse::decode(&res_bytes[..]) {
                Ok(r) => Some(r.value),
                Err(_) => None,
            }
        }
        Err(_) => None,
    }
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
    unsafe {
        veld_output_set(data.as_ptr() as u64, data.len() as u64);
    }
}
