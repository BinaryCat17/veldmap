use std::sync::Arc;

#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(uniffi::Object, Debug, Clone)]
pub struct DemTile {
    pub id: Option<TileId>,
    pub heights: Vec<f32>,
    pub width: u64,
    pub height: u64,
    pub min_alt: f32,
    pub max_alt: f32,
}

#[uniffi::export]
impl DemTile {
    #[uniffi::constructor]
    pub fn new(id: Option<TileId>, heights: Vec<f32>, width: u64, height: u64, min_alt: f32, max_alt: f32) -> Arc<Self> {
        Arc::new(Self { id, heights, width, height, min_alt, max_alt })
    }
}
