use veldmap_gis_api::storage::{GetTileRequest, GetTileResponse};
use veldmap_gis_api::common::{DemTile, TileId};
use crate::{LocalConfig, LocalState};

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState)
}

pub fn handle_get_tile(_state: &LocalState, request: GetTileRequest) -> anyhow::Result<GetTileResponse> {
    let tile_id = request.id.unwrap_or(TileId { x: 0, y: 0, z: 0 });

    let tile = DemTile {
        id: Some(tile_id),
        heights: vec![0.0; 256 * 256],
        width: 256,
        height: 256,
        min_alt: 0.0,
        max_alt: 0.0,
    };

    Ok(GetTileResponse {
        tile: Some(tile),
        error: String::new(),
    })
}