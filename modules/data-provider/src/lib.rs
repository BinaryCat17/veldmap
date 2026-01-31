mod cdse;

use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use veldmap_rust_rpc::dataprovider::{SearchRequest, DownloadRequest, ListPathRequest};
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match request.method.as_str() {
            "search" => {
                let search_req = SearchRequest::decode(&request.payload[..]).unwrap();
                match cdse::search(search_req) {
                    Ok(res) => (res.encode_to_vec(), String::new()),
                    Err(e) => (Vec::new(), e.to_string()),
                }
            }
            "download" => {
                let download_req = DownloadRequest::decode(&request.payload[..]).unwrap();
                match cdse::download(download_req) {
                    Ok(res) => (res.encode_to_vec(), String::new()),
                    Err(e) => (Vec::new(), e.to_string()),
                }
            }
            "list_path" => {
                let list_req = ListPathRequest::decode(&request.payload[..]).unwrap();
                match cdse::list_path(list_req) {
                    Ok(res) => (res.encode_to_vec(), String::new()),
                    Err(e) => (Vec::new(), e.to_string()),
                }
            }
            _ => (Vec::new(), format!("Method {} not found in data-provider", request.method)),
        }
    }));

    let (payload, error) = match res {
        Ok(val) => val,
        Err(_) => (Vec::new(), "Plugin panicked during execution".to_string()),
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}
