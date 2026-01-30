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

#[derive(uniffi::Record, Clone, Debug)]
pub struct DataProduct {
    pub name: String,
    pub path: String,
    pub grid_id: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchFilter {
    pub name: String,
    pub value: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ListResult {
    pub items: Vec<String>,
    pub next_token: Option<String>,
}

#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait RemoteDataSource: Send + Sync {
    async fn search(&self, query: String, filters: Vec<SearchFilter>) -> Result<Vec<DataProduct>, String>;
    async fn list_path(&self, prefix: String, token: Option<String>) -> Result<ListResult, String>;
    async fn download(&self, key: String, destination: String) -> Result<(), String>;
}
