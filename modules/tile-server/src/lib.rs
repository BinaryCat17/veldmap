use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let _request = match RpcRequest::decode(&input[..]) {
        Ok(r) => r,
        Err(e) => return Err(anyhow::anyhow!("Failed to decode RpcRequest: {}", e).into()),
    };
    
    let response = RpcResponse {
        payload: Vec::new(),
        error: "Tile server logic not implemented".to_string(),
        sync: None,
    };
    
    let mut out = Vec::new();
    response.encode(&mut out)?;
    Ok(out)
}
