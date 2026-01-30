use std::sync::Arc;
use crate::common_module::{TileId, DemTile};

pub trait TerrainProvider: Send + Sync {
    fn get_tile(&self, id: TileId) -> Result<Arc<DemTile>, String>;
    fn get_geoid(&self) -> Result<Arc<DemTile>, String>;
}
