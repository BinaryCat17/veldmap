use std::sync::Arc;

#[derive(uniffi::Record, Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct TileId {
    pub z: u32,
    pub x: u32,
    pub y: u32,
}

#[derive(uniffi::Object)]
pub struct DemTile {
    pub heights: Vec<f32>,
    pub width: u64,
    pub height: u64,
}

#[uniffi::export]
impl DemTile {
    #[uniffi::constructor]
    pub fn new(heights: Vec<f32>, width: u64, height: u64) -> Self {
        Self { heights, width, height }
    }
}

#[uniffi::export(callback_interface)]
pub trait TerrainProvider: Send + Sync {
    fn get_tile(&self, id: TileId) -> Result<Arc<DemTile>, String>;
    fn get_geoid(&self) -> Result<Arc<DemTile>, String>;
}

#[uniffi::export(callback_interface)]
pub trait ImageryProvider: Send + Sync {
    fn get_tile(&self, id: TileId) -> Result<Vec<u8>, String>;
}
