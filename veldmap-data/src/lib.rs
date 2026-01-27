use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, BufReader, BufRead};
use tiff::decoder::{Decoder, DecodingResult};
use async_trait::async_trait;
pub use veldmap_core::{TileId, DemTile, TerrainProvider};

pub struct Config {
    pub base_path: PathBuf,
    pub use_cache: bool,
    pub offline_only: bool,
}

pub struct DataProvider {
    config: Config,
}

impl DataProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn find_local_path(&self, id: TileId, folder: &str, ext: &str) -> PathBuf {
        self.config.base_path
            .join(folder)
            .join(id.z.to_string())
            .join(id.x.to_string())
            .join(format!("{}.{}", id.y, ext))
    }

    fn load_tiff(&self, path: &Path) -> anyhow::Result<DemTile> {
        let file = File::open(path)?;
        let mut decoder = Decoder::new(file)?;
        let (w, h) = decoder.dimensions()?;
        let data: Vec<f32> = match decoder.read_image()? {
            DecodingResult::F32(v) => v,
            DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
            DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
            _ => return Err(anyhow::anyhow!("Unsupported TIFF format")),
        };
        Ok(DemTile { heights: data, width: w as usize, height: h as usize })
    }

    fn load_pgm(&self, path: &Path) -> anyhow::Result<DemTile> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            if !line.starts_with('#') { break; }
        }
        let dims: Vec<usize> = line.split_whitespace().map(|s| s.parse().unwrap()).collect();
        let w = dims[0]; let h = dims[1];
        line.clear();
        reader.read_line(&mut line)?;
        let mut raw_data = Vec::new();
        reader.read_to_end(&mut raw_data)?;
        let mut heights = Vec::with_capacity(w * h);
        for i in 0..(w * h) {
            let idx = i * 2;
            if idx + 1 < raw_data.len() {
                let val = u16::from_be_bytes([raw_data[idx], raw_data[idx+1]]);
                heights.push((val as f32 - 32768.0) * 0.01);
            }
        }
        Ok(DemTile { heights, width: w, height: h })
    }
}

#[async_trait]
impl TerrainProvider for DataProvider {
    async fn get_tile(&self, id: TileId) -> anyhow::Result<DemTile> {
        let path = self.find_local_path(id, "dem", "tif");
        if path.exists() { return self.load_tiff(&path); }
        let rostov = self.config.base_path.join("dem").join("rostov_tile.tif");
        if rostov.exists() { return self.load_tiff(&rostov); }
        Err(anyhow::anyhow!("Tile not found"))
    }

    async fn get_geoid(&self) -> anyhow::Result<DemTile> {
        let path = self.config.base_path.join("geoids/egm2008-5.pgm");
        self.load_pgm(&path)
    }
}