#[cfg(feature = "pdk")]
use crate::rpc::services::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

// --- VELD SYSTEM ABI (V1) ---
#[cfg(feature = "pdk")]
#[allow(dead_code)]
extern "C" {
    fn veld_host_call(ptr: u64, len: u64) -> u64;
    fn veld_alloc(len: u64) -> u64;
    fn veld_free(ptr: u64);
    fn veld_ptr_len(ptr: u64) -> u64;
    fn veld_gpu_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_gpu_read(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_get_info(key_ptr: u64, key_len: u64) -> u64;
    fn veld_http_request(req_ptr: u64, req_len: u64, body_ptr: u64, body_len: u64) -> u64;
    fn veld_load_u8(p: u64) -> u8;
    fn veld_input_len() -> u64;
    fn veld_input_load_u8(i: u64) -> u8;
    fn veld_output_set(p: u64, n: u64);
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

        let len = veld_ptr_len(res_ptr);
        let mut res_buf = vec![0u8; len as usize];
        for i in 0..len { res_buf[i as usize] = veld_load_u8(res_ptr + i); }

        let response = RpcResponse::decode(&res_buf[..])?;
        if !response.error.is_empty() { return Err(anyhow::anyhow!(response.error)); }
        Ok(response.payload)
    }
}

#[cfg(feature = "pdk")]
pub fn gpu_write_resource(id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
    unsafe { veld_gpu_write(id, offset, data.as_ptr() as u64, data.len() as u64); }
    Ok(())
}

#[cfg(feature = "pdk")]
pub fn gpu_read_resource(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    let mut buf = vec![0u8; size as usize];
    unsafe { veld_gpu_read(id, offset, buf.as_mut_ptr() as u64, size); }
    Ok(buf)
}

#[cfg(feature = "pdk")]
pub fn get_config(key: &str) -> Option<String> {
    unsafe {
        let res_ptr = veld_get_info(key.as_ptr() as u64, key.len() as u64);
        if res_ptr == 0 { return None; }
        
        let len = veld_ptr_len(res_ptr);
        let mut buf = vec![0u8; len as usize];
        for i in 0..len { buf[i as usize] = veld_load_u8(res_ptr + i); }
        String::from_utf8(buf).ok()
    }
}

#[cfg(feature = "pdk")]
pub fn http_request(json_req: &str, body: Option<&[u8]>) -> anyhow::Result<(u32, Vec<u8>)> {
    unsafe {
        let (b_ptr, b_len) = body.map(|b| (b.as_ptr() as u64, b.len() as u64)).unwrap_or((0, 0));
        let res_ptr = veld_http_request(json_req.as_ptr() as u64, json_req.len() as u64, b_ptr, b_len);
        
        if res_ptr == 0 { return Err(anyhow::anyhow!("HTTP failed")); }
        
        // В нашем ABI статус можно получить через специальный вызов
        extern "C" { fn veld_http_status_get() -> i32; }
        let status = veld_http_status_get() as u32;
        
        let len = veld_ptr_len(res_ptr);
        let mut buf = vec![0u8; len as usize];
        for i in 0..len { buf[i as usize] = veld_load_u8(res_ptr + i); }
        Ok((status, buf))
    }
}

pub fn load_input() -> Vec<u8> {
    unsafe {
        let len = veld_input_len();
        let mut buf = vec![0u8; len as usize];
        for i in 0..len { buf[i as usize] = veld_input_load_u8(i); }
        buf
    }
}

pub fn store_output(data: Vec<u8>) {
    unsafe {
        veld_output_set(data.as_ptr() as u64, data.len() as u64);
        std::mem::forget(data); 
    }
}
