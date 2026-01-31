use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct LocalConfig {
    pub node_id: String,
}

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let _config: LocalConfig = match config::get("config") {
        Ok(Some(c)) => serde_json::from_str(&c)?,
        _ => return Err(anyhow::anyhow!("Configuration not found for core-wasm").into()),
    };

    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.service.as_str() {
        "core" => {
            match request.method.as_str() {
                "status" => (Vec::new(), String::new()),
                _ => (Vec::new(), format!("Method {} not found in core", request.method)),
            }
        }
        _ => {
            // Маршрутизация к другим плагинам через хост
            unsafe {
                let res_bytes = host_call_plugin(request.encode_to_vec())?;
                let response = RpcResponse::decode(&res_bytes[..])?;
                (response.payload, response.error)
            }
        }
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}

#[host_fn]
extern "ExtismHost" {
    fn host_call_plugin(request: Vec<u8>) -> Vec<u8>;
}
