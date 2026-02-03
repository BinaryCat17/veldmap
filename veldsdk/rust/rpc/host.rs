#[cfg(feature = "pdk")]
use extism_pdk::*;
#[cfg(feature = "pdk")]
use crate::rpc::services::{RpcRequest, RpcResponse};
#[cfg(feature = "pdk")]
use prost::Message;

#[cfg(feature = "pdk")]
pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let request = RpcRequest {
        service: service.to_string(),
        method: method.to_string(),
        payload,
        sync: None,
    };
    
    let req_buf = request.encode_to_vec();
    let mem = Memory::from_bytes(&req_buf)?;
    
    let res_ptr = unsafe { veldmap_host_call(mem.offset() as i64) };
    mem.free();
    
    if res_ptr == 0 {
        return Err(anyhow::anyhow!("Host call to {}:{} failed", service, method));
    }

    let res_mem = Memory::find(res_ptr as u64)
        .ok_or_else(|| anyhow::anyhow!("Failed to find response memory block"))?;
    
    let res_buf = res_mem.to_vec();
    res_mem.free();

    let response = RpcResponse::decode(&res_buf[..])?;
    if !response.error.is_empty() {
        return Err(anyhow::anyhow!(response.error));
    }
    
    Ok(response.payload)
}

#[cfg(feature = "pdk")]
pub fn gpu_write_resource(id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
    let mem = Memory::from_bytes(data)?;
    unsafe {
        veld_gpu_write_resource(id as i64, offset as i64, mem.offset() as i64, data.len() as i64);
    }
    mem.free();
    Ok(())
}

#[cfg(feature = "pdk")]
pub fn gpu_read_resource(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    let mem = Memory::new(size as usize)?;
    unsafe {
        veld_gpu_read_resource(id as i64, offset as i64, mem.offset() as i64, size as i64);
    }
    let data = mem.to_vec();
    mem.free();
    Ok(data)
}

#[cfg(feature = "pdk")]
extern "C" {
    fn veldmap_host_call(ptr: i64) -> i64;
    fn veld_gpu_write_resource(id: i64, offset: i64, ptr: i64, len: i64);
    fn veld_gpu_read_resource(id: i64, offset: i64, ptr: i64, len: i64);
}
