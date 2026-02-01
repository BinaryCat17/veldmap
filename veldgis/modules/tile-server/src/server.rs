use veldmap_gis_api::tileserver::{TileRequest, TileResponse};
use crate::{LocalConfig, LocalState};

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState)
}

pub fn handle_tile_request(_state: &LocalState, _req: TileRequest) -> anyhow::Result<TileResponse> {
    // В будущем здесь будет логика тайлового сервера
    Ok(TileResponse {
        data: Vec::new(),
        content_type: "image/png".to_string(),
        error: "Tile server logic not implemented yet".to_string(),
    })
}