use veldmap_rust_rpc::storage::{GetTileRequest, GetTileResponse};
use veldmap_rust_rpc::common::{DemTile, TileId};
use prost::Message;

pub fn handle_get_tile(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
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
    };

    Ok(response.encode_to_vec())
}
