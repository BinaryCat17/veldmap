use async_trait::async_trait;

// Константы планеты
pub const WGS84_A: f64 = 6378137.0;
pub const WGS84_B: f64 = 6356752.314245;

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct TileId {
    pub z: u32,
    pub x: u32,
    pub y: u32,
}

pub struct DemTile {
    pub heights: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

#[async_trait]
pub trait TerrainProvider: Send + Sync {
    async fn get_tile(&self, id: TileId) -> anyhow::Result<DemTile>;
    async fn get_geoid(&self) -> anyhow::Result<DemTile>;
}

#[async_trait]
pub trait ImageryProvider: Send + Sync {
    async fn get_tile(&self, id: TileId) -> anyhow::Result<Vec<u8>>;
}
