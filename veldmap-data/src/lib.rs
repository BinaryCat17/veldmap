mod provider;

use std::path::PathBuf;
use std::sync::Arc;
use veldmap_core::data_module::TerrainProvider;
use crate::provider::DataProvider;

pub struct Config {
    pub server_url: String,
    pub cache_path: Option<PathBuf>,
    pub use_cache: bool,
}

/// Фабрика для создания провайдера данных.
pub fn create_data_provider(config: Config) -> Arc<dyn TerrainProvider> {
    Arc::new(DataProvider::new(config))
}
