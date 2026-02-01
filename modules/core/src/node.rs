use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use veldmap_rust_rpc::host::call_service;
use prost::Message;
use extism_pdk::*;
use crate::{LocalConfig, LocalState};

pub(crate) fn module_init(cfg: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState { config: cfg })
}

pub(crate) fn custom_handle_rpc(_state: &LocalState, input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    if request.service == "core" {
        if request.method == "status" {
            let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
            return Ok(response.encode_to_vec());
        }
        return Ok(RpcResponse { 
            payload: Vec::new(), 
            error: format!("Method {} not found in core", request.method), 
            sync: None 
        }.encode_to_vec());
    }

    // Маршрутизация к другим плагинам через хост с использованием стандартной обертки
    let (payload, error) = match call_service(&request.service, &request.method, request.payload) {
        Ok(p) => (p, String::new()),
        Err(e) => (Vec::new(), e.to_string()),
    };
    
    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}