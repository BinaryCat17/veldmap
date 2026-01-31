use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse, GetTileRequest, GetTileResponse};
use veldmap_rust_rpc::common::{DemTile, TileId};
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "get_tile" => {
            match handle_get_tile(request.payload) {
                Ok(p) => (p, String::new()),
                Err(e) => (Vec::new(), e.to_string()),
            }
        }
        "get_geoid" => {
            (Vec::new(), "Geoid not implemented".to_string())
        }
        _ => (Vec::new(), format!("Method {} not found in local-storage", request.method)),
    };

    let response = RpcResponse {
        payload,
        error,
        sync: None,
    };

    let mut out = Vec::new();
    response.encode(&mut out)?;
    Ok(out)
}

fn handle_get_tile(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let request = GetTileRequest::decode(&payload[..])?;
    let tile_id = request.id.unwrap_or(TileId { x: 0, y: 0, z: 0 });

    let tile = DemTile {
        id: Some(tile_id),
        heights: vec![0.0; 256 * 256],
        width: 256,
        height: 256,
        min_alt: 0.0,
        max_alt: 0.0,
    };

    let response = GetTileResponse {
        tile: Some(tile),
        error: String::new(),
        sync: None,
    };

    let mut out = Vec::new();
    response.encode(&mut out)?;
    Ok(out)
}