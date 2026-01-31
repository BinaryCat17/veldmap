mod cdse;

use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse, SearchRequest};
use prost::Message;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct LocalConfig {
    pub api_endpoint: String,
    // Другие специфичные для модуля настройки
}

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    // Загружаем конфиг, который передал нам Host (ядро)
    let _config: LocalConfig = match config::get("config") {
        Ok(Some(c)) => serde_json::from_str(&c)?,
        _ => return Err(anyhow::anyhow!("Configuration not found for module").into()),
    };

    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "search" => {
            let search_req = SearchRequest::decode(&request.payload[..])?;
            match cdse::search(search_req) {
                Ok(res) => (res.encode_to_vec(), String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            }
        }
        "download" => {
            let download_req = veldmap_rust_rpc::services::DownloadRequest::decode(&request.payload[..])?;
            match cdse::download(download_req) {
                Ok(res) => (res.encode_to_vec(), String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            }
        }
        _ => (Vec::new(), format!("Method {} not found in data-provider", request.method)),
    };

    let response = RpcResponse {
        payload,
        error,
        sync: None,
    };

    Ok(response.encode_to_vec())
}