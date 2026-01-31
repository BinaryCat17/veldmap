mod math;

use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct LocalConfig {
    // В данном модуле пока нет специфичных настроек
}

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let _config: LocalConfig = match config::get("config") {
        Ok(Some(c)) => serde_json::from_str(&c)?,
        _ => LocalConfig {}, // По умолчанию пустой конфиг
    };

    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "lla_to_ecef" => {
            match math::handle_lla_to_ecef(request.payload) {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            }
        }
        _ => (Vec::new(), format!("Method {} not found in geo-math", request.method)),
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}