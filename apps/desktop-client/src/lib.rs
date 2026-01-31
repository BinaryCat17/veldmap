use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "init" => (Vec::new(), String::new()),
        _ => (Vec::new(), format!("Method {} not found in desktop-client", request.method)),
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}
