#[cfg(target_arch = "wasm32")]
use extism_pdk::*;
#[cfg(target_arch = "wasm32")]
use crate::services::{RpcRequest, RpcResponse};
#[cfg(target_arch = "wasm32")]
use prost::Message;

#[cfg(target_arch = "wasm32")]
pub fn call_service(service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let request = RpcRequest {
        service: service.to_string(),
        method: method.to_string(),
        payload,
        sync: None,
    };
    
    let mut req_buf = Vec::new();
    request.encode(&mut req_buf)?;

    // Вызываем хост-функцию, которую предоставило ядро
    let res_ptr: i64 = unsafe { veldmap_host_call(req_buf.as_ptr() as i64) };
    
    // В Extism PDK работа с возвращаемыми указателями из хост-функций 
    // обычно требует Memory объекта.
    // Но для простоты предположим, что мы используем стандартный механизм Extism.
    
    // ВАЖНО: Это упрощенный пример. Реальный PDK может потребовать 
    // использования extism_pdk::native::...
    
    let res_buf = Memory::find(res_ptr as u64)
        .ok_or_else(|| anyhow::anyhow!("Failed to find response memory"))?
        .to_vec();

    let response = RpcResponse::decode(&res_buf[..])?;
    if !response.error.is_empty() {
        return Err(anyhow::anyhow!(response.error));
    }
    
    Ok(response.payload)
}

#[cfg(target_arch = "wasm32")]
extern "C" {
    fn veldmap_host_call(ptr: i64) -> i64;
}
