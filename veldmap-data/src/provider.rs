use std::path::PathBuf;
use std::sync::Arc;
use std::fs;
use serde::{Deserialize, Serialize};
use veldmap_core::data_module::{TerrainProvider, TileId, DemTile};
use crate::Config;

#[derive(Serialize, Deserialize)]
struct DemTileResponse {
    heights: Vec<f32>,
    width: u64,
    height: u64,
}

pub(crate) struct DataProvider {
    pub(crate) config: Config,
}

impl DataProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn get_cache_path(&self, id: Option<TileId>) -> Option<PathBuf> {
        self.config.cache_path.as_ref().map(|p| {
            if let Some(id) = id {
                p.join(format!("{}_{}_{}.json", id.z, id.x, id.y))
            } else {
                p.join("geoid.json")
            }
        })
    }

    fn fetch_from_server(&self, url: &str) -> Result<DemTile, String> {
        let response = ureq::get(url)
            .call()
            .map_err(|e| e.to_string())?;
        
        let data: DemTileResponse = response.into_json()
            .map_err(|e| e.to_string())?;
            
        Ok(DemTile {
            heights: data.heights,
            width: data.width,
            height: data.height,
        })
    }
}

impl TerrainProvider for DataProvider {
    fn get_tile(&self, id: TileId) -> Result<Arc<DemTile>, String> {
        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(Some(id)) {
                if path.exists() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(data) = serde_json::from_str::<DemTileResponse>(&content) {
                            return Ok(Arc::new(DemTile { heights: data.heights, width: data.width, height: data.height }));
                        }
                    }
                }
            }
        }

        let url = format!("{}/v1/terrain/{}/{}/{}", self.config.server_url, id.z, id.x, id.y);
        let tile = self.fetch_from_server(&url)?;

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

        Ok(Arc::new(tile))
    }

    fn get_geoid(&self) -> Result<Arc<DemTile>, String> {
        if self.config.use_cache {
            if let Some(path) = self.get_cache_path(None) {
                if path.exists() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(data) = serde_json::from_str::<DemTileResponse>(&content) {
                            return Ok(Arc::new(DemTile { heights: data.heights, width: data.width, height: data.height }));
                        }
                    }
                }
            }
        }

        let url = format!("{}/v1/geoid", self.config.server_url);
        let tile = self.fetch_from_server(&url)?;

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

        Ok(Arc::new(tile))
    }
}