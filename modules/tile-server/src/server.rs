use veldmap_rust_rpc::services::RpcResponse;
use prost::Message;

pub fn handle_tile_request(_payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    // В будущем здесь будет логика тайлового сервера
    let response = RpcResponse {
        payload: Vec::new(),
        error: "Tile server logic not implemented yet".to_string(),
        sync: None,
    };
    Ok(response.encode_to_vec())
}
