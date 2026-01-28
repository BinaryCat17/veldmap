use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, BufReader, BufRead};
use tiff::decoder::{Decoder, DecodingResult};
use std::sync::Arc;
pub use veldmap_core::data_module::{TerrainProvider, TileId, DemTile};

pub struct Config {
    pub base_path: PathBuf,
    pub use_cache: bool,
    pub offline_only: bool,
}

pub struct DataProvider {
    config: Config,
}

/// Фабрика для создания провайдера данных.
pub fn create_data_provider(config: Config) -> Arc<dyn TerrainProvider> {
    Arc::new(DataProvider { config })
}

impl DataProvider {
    fn find_local_path(&self, id: TileId, folder: &str, ext: &str) -> PathBuf {
        self.config.base_path
            .join(folder)
            .join(id.z.to_string())
            .join(id.x.to_string())
            .join(format!("{}.{}", id.y, ext))
    }

    fn load_tiff(&self, path: &Path) -> Result<DemTile, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut decoder = Decoder::new(file).map_err(|e| e.to_string())?;
        let (w, h) = decoder.dimensions().map_err(|e| e.to_string())?;
        let data: Vec<f32> = match decoder.read_image().map_err(|e| e.to_string())? {
            DecodingResult::F32(v) => v,
            DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
            DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
            _ => return Err("Unsupported TIFF format".to_string()),
        };
        Ok(DemTile { heights: data, width: w as u64, height: h as u64 })
    }

    fn load_pgm(&self, path: &Path) -> Result<DemTile, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        loop {
            line.clear();
            reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if !line.starts_with('#') { break; }
        }
        let dims: Vec<usize> = line.split_whitespace().map(|s| s.parse().unwrap()).collect();
        let w = dims[0]; let h = dims[1];
        line.clear();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let mut raw_data = Vec::new();
        reader.read_to_end(&mut raw_data).map_err(|e| e.to_string())?;
        let mut heights = Vec::with_capacity(w * h);
        for i in 0..(w * h) {
            let idx = i * 2;
            if idx + 1 < raw_data.len() {
                let val = u16::from_be_bytes([raw_data[idx], raw_data[idx+1]]);
                heights.push((val as f32 - 32768.0) * 0.01);
            }
        }
        Ok(DemTile { heights, width: w as u64, height: h as u64 })
    }
}

impl TerrainProvider for DataProvider {
    fn get_tile(&self, id: TileId) -> Result<Arc<DemTile>, String> {
        let path = self.find_local_path(id, "dem", "tif");
        if path.exists() { return self.load_tiff(&path).map(Arc::new); }
        let rostov = self.config.base_path.join("dem").join("rostov_tile.tif");
        if rostov.exists() { return self.load_tiff(&rostov).map(Arc::new); }
        Err("Tile not found".to_string())
    }

    fn get_geoid(&self) -> Result<Arc<DemTile>, String> {
        let path = self.config.base_path.join("geoids/egm2008-5.pgm");
        self.load_pgm(&path).map(Arc::new)
    }
}
