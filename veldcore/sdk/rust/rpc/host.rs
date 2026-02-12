#[cfg(feature = "pdk")]
use crate::rpc::core::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

// --- VELD SYSTEM ABI (V2) ---
#[cfg(feature = "pdk")]
#[allow(dead_code)]
extern "C" {
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    fn veld_gpu_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_gpu_read(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_gpu_create_buffer(usage: u32, ptr: u64, len: u64, readonly: u32) -> u64;
    fn veld_gpu_freeze_resource(id: u64) -> u32;
    fn veld_get_info(key_ptr: u64, key_len: u64) -> u64;
    fn veld_http_request(req_ptr: u64, req_len: u64, body_ptr: u64, body_len: u64, status_ptr: u64) -> u64;
    fn veld_load_u8(p: u64) -> u8;
    fn veld_input_len() -> u64;
    fn veld_input_copy(p: u64, n: u64);
    fn veld_output_set(p: u64, n: u64);
    fn veld_free(p: u64, n: u64);
    fn veld_generate_uuid(p: u64);
}

#[cfg(feature = "pdk")]
pub fn generate_uuid() -> String {
    let mut buf = [0u8; 36];
    unsafe { veld_generate_uuid(buf.as_mut_ptr() as u64); }
    String::from_utf8_lossy(&buf).into_owned()
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
    };
    
    let req_buf = request.encode_to_vec();
    unsafe {
        let res_ptr = veld_host_call(req_buf.as_ptr() as u64, req_buf.len() as u64);
        if res_ptr == 0 { return Err(anyhow::anyhow!("Veld System Call failed")); }

        let combined = res_ptr;
        let ptr = (combined & 0xFFFFFFFF) as *mut u8;
        let len = (combined >> 32) as usize;
        
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
        veld_gpu_write(id, offset, data.as_ptr() as u64, data.len() as u64); 
    }
    Ok(())
}

#[cfg(feature = "pdk")]
pub fn gpu_read_resource(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let mut buf = vec![0u8; size as usize];
        veld_gpu_read(id, offset, buf.as_mut_ptr() as u64, size);
        Ok(buf)
    }
}

#[cfg(feature = "pdk")]
pub fn gpu_create_buffer(usage: u32, data: &[u8], readonly: bool) -> u64 {
    unsafe {
        veld_gpu_create_buffer(usage, data.as_ptr() as u64, data.len() as u64, if readonly { 1 } else { 0 })
    }
}

#[cfg(feature = "pdk")]
pub fn gpu_freeze_resource(id: u64) -> bool {
    unsafe {
        veld_gpu_freeze_resource(id) != 0
    }
}

#[cfg(feature = "pdk")]
pub fn get_config(key: &str) -> Option<String> {
    unsafe {
        let res_ptr = veld_get_info(key.as_ptr() as u64, key.len() as u64);
        if res_ptr == 0 { return None; }
        
        let ptr = (res_ptr & 0xFFFFFFFF) as *mut u8;
        let len = (res_ptr >> 32) as usize;
        
        let buf = std::slice::from_raw_parts(ptr, len);
        let s = String::from_utf8_lossy(buf).into_owned();
        
        veld_free_wasm(ptr as u64, len as u64);
        Some(s)
    }
}

#[cfg(feature = "pdk")]
pub fn http_request(json_req: &str, body: Option<&[u8]>) -> anyhow::Result<(u32, Vec<u8>)> {
    unsafe {
        let mut status: u32 = 0;
        let (b_ptr, b_len) = body.map(|b| (b.as_ptr() as u64, b.len() as u64)).unwrap_or((0, 0));
        
        let res_ptr = veld_http_request(
            json_req.as_ptr() as u64, 
            json_req.len() as u64, 
            b_ptr, 
            b_len, 
            &mut status as *mut u32 as u64
        );
        
        if res_ptr == 0 { return Err(anyhow::anyhow!("HTTP failed")); }
        
        let ptr = (res_ptr & 0xFFFFFFFF) as *mut u8;
        let len = (res_ptr >> 32) as usize;
        
        let buf = std::slice::from_raw_parts(ptr, len).to_vec();
        veld_free_wasm(ptr as u64, len as u64);
        
        Ok((status, buf))
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


