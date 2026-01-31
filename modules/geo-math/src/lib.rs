mod math;

use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use veldmap_rust_rpc::geomath::{Lla, Ecef};
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "lla_to_ecef" => {
            match handle_lla_to_ecef(request.payload) {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            }
        }
        _ => (Vec::new(), format!("Method {} not found in geo-math", request.method)),
    };

    let response = RpcResponse {
        payload,
        error,
        sync: None,
    };

    Ok(response.encode_to_vec())
}

fn handle_lla_to_ecef(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let lla = Lla::decode(&payload[..])?;
    let (x, y, z) = math::lla_to_ecef(lla.lat, lla.lon, lla.alt);
    
    let ecef = Ecef { x, y, z };
    Ok(ecef.encode_to_vec())
}
