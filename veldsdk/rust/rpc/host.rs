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
    
    // Делаем вызов
    let res_ptr = unsafe { veldmap_host_call(mem.offset() as i64) };
    
    // СРАЗУ освобождаем память запроса, она больше не нужна
    mem.free();
    
    if res_ptr == 0 {
        return Err(anyhow::anyhow!("Host call to {}:{} failed (plugin returned null, possibly execution error or OOM)", service, method));
    }

    let res_mem = Memory::find(res_ptr as u64)
        .ok_or_else(|| anyhow::anyhow!("Failed to find response memory block"))?;
    
    let res_buf = res_mem.to_vec();
    // Освобождаем память ответа
    res_mem.free();

    let response = RpcResponse::decode(&res_buf[..])?;
    if !response.error.is_empty() {
        return Err(anyhow::anyhow!(response.error));
    }
    
    Ok(response.payload)
}

#[cfg(feature = "pdk")]

extern "C" {

    fn veldmap_host_call(ptr: i64) -> i64;

}
