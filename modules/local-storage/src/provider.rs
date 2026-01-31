use std::path::PathBuf;
use std::sync::Arc;
use std::fs;
use serde::{Deserialize, Serialize};
use veldmap_core::common_module::{TileId, DemTile};
use veldmap_core::local_storage_module::TerrainProvider;

#[derive(Serialize, Deserialize)]
struct DemTileResponse {
    heights: Vec<f32>,
    width: u64,
    height: u64,
}

pub(crate) struct DataProvider {
    pub(crate) config: crate::LocalConfig,
}

impl DataProvider {
    pub fn new(config: crate::LocalConfig) -> Self {
        Self { config }
    }

    fn get_cache_path(&self, id: Option<TileId>) -> Option<PathBuf> {
        self.config.cache_path.as_ref().map(|p: &PathBuf| {
            if let Some(id) = id {
                p.join(format!("{}_{}_{}.json", id.z, id.x, id.y))
            } else {
                p.join("geoid.json")
            }
        })
    }

    fn fetch_from_server(&self, id: Option<TileId>, url: &str) -> Result<Arc<DemTile>, String> {
        let response = ureq::get(url)
            .call()
            .map_err(|e| e.to_string())?;
        
        let data: DemTileResponse = serde_json::from_reader(response.into_reader())
            .map_err(|e| e.to_string())?;

        let min_alt = *data.heights.iter().min_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
        let max_alt = *data.heights.iter().max_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
            
        Ok(DemTile::new(id, data.heights, data.width, data.height, min_alt, max_alt))
    }
}

impl TerrainProvider for DataProvider {
    fn get_tile(&self, id: TileId) -> Result<Arc<DemTile>, String> {
        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(Some(id)) {
                if path.exists() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(data) = serde_json::from_str::<DemTileResponse>(&content) {
                            let min_alt = *data.heights.iter().min_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
                            let max_alt = *data.heights.iter().max_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
                            return Ok(DemTile::new(Some(id), data.heights, data.width, data.height, min_alt, max_alt));
                        }
                    }
                }
            }
        }

        let url = format!("{}/v1/terrain/{}/{}/{}", self.config.server_url, id.z, id.x, id.y);
        let tile = self.fetch_from_server(Some(id), &url)?;

        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(Some(id)) {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).ok();
                }
                let response = DemTileResponse { heights: tile.heights.clone(), width: tile.width, height: tile.height };
                if let Ok(content) = serde_json::to_string(&response) {
                    fs::write(path, content).ok();
                }
            }
        }

        Ok(tile)
    }

    fn get_geoid(&self) -> Result<Arc<DemTile>, String> {
        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(None) {
                if path.exists() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(data) = serde_json::from_str::<DemTileResponse>(&content) {
                            let min_alt = *data.heights.iter().min_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
                            let max_alt = *data.heights.iter().max_by(|a: &&f32, b: &&f32| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
                            return Ok(DemTile::new(None, data.heights, data.width, data.height, min_alt, max_alt));
                        }
                    }
                }
            }
        }

        let url = format!("{}/v1/geoid", self.config.server_url);
        let tile = self.fetch_from_server(None, &url)?;

        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(None) {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).ok();
                }
                let response = DemTileResponse { heights: tile.heights.clone(), width: tile.width, height: tile.height };
                if let Ok(content) = serde_json::to_string(&response) {
                    fs::write(path, content).ok();
                }
            }
        }

        Ok(tile)
    }
}
