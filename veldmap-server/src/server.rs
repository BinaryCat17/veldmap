use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, BufReader, BufRead};
use tiff::decoder::{Decoder, DecodingResult};
use veldmap_core::common_module::{TileId, DemTile};
use veldmap_core::server_module::{VeldMapServer, ServerConfig};
use axum::{
    extract::{Path as AxumPath, State},
    routing::get,
    Router,
    response::IntoResponse,
    Json,
    http::StatusCode,
};
use serde::Serialize;

#[derive(Serialize)]
struct DemTileResponse {
    heights: Vec<f32>,
    width: u64,
    height: u64,
}

pub(crate) struct VeldMapServerImpl {
    pub(crate) config: ServerConfig,
}

impl VeldMapServerImpl {
    fn find_local_path(&self, id: TileId, folder: &str, ext: &str) -> PathBuf {
        self.config.data_path
            .join(folder)
            .join(id.z.to_string())
            .join(id.x.to_string())
            .join(format!("{}.{}", id.y, ext))
    }

    fn process_raw_data(&self, id: Option<TileId>, data: Vec<f32>, w: usize, h: usize) -> Result<Arc<DemTile>, String> {
        let target_w = 256;
        let target_h = 256;

        let (final_data, final_w, final_h) = if w == target_w && h == target_h {
             (data, w, h)
        } else if w > target_w || h > target_h {
            let mut resized = Vec::with_capacity(target_w * target_h);
            let x_ratio = w as f32 / target_w as f32;
            let y_ratio = h as f32 / target_h as f32;
            for y in 0..target_h {
                let src_y = (y as f32 * y_ratio).floor() as usize;
                for x in 0..target_w {
                    let src_x = (x as f32 * x_ratio).floor() as usize;
                    let idx = src_y * w + src_x;
                    resized.push(if idx < data.len() { data[idx] } else { 0.0 });
                }
            }
            (resized, target_w, target_h)
        } else {
            (data, w, h)
        };

        let min_alt = *final_data.iter().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);
        let max_alt = *final_data.iter().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(&0.0);

        Ok(DemTile::new(id, final_data, final_w as u64, final_h as u64, min_alt, max_alt))
    }

    fn load_tiff(&self, id: Option<TileId>, path: &Path) -> Result<Arc<DemTile>, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut decoder = Decoder::new(file).map_err(|e| e.to_string())?;
        let (w, h) = decoder.dimensions().map_err(|e| e.to_string())?;
        
        let data: Vec<f32> = match decoder.read_image().map_err(|e| e.to_string())? {
            DecodingResult::F32(v) => v,
            DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
            DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
            _ => return Err("Unsupported TIFF format".to_string()),
        };

        self.process_raw_data(id, data, w as usize, h as usize)
    }

    fn load_pgm(&self, path: &Path) -> Result<Arc<DemTile>, String> {
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
        
        self.process_raw_data(None, heights, w, h)
    }
}

impl VeldMapServer for VeldMapServerImpl {
    fn run(&self) -> anyhow::Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let app = Router::new()
            .route("/", get(|| async { "VeldMap Data Server is running" }))
            .route("/v1/terrain/:z/:x/:y", get(get_terrain_tile))
            .route("/v1/geoid", get(get_geoid))
            .with_state(self.clone_internal());

        tracing::info!("Listening on {} with data at {:?}", self.config.addr, self.config.data_path);
        
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind(self.config.addr).await?;
            axum::serve(listener, app).await?;
            Ok::<(), anyhow::Error>(())
        })?;
        
        Ok(())
    }
}

impl VeldMapServerImpl {
    fn clone_internal(&self) -> Arc<Self> {
        Arc::new(Self {
            config: ServerConfig {
                addr: self.config.addr,
                data_path: self.config.data_path.clone(),
            }
        })
    }
}

async fn get_terrain_tile(
    AxumPath((z, x, y)): AxumPath<(u32, u32, u32)>,
    State(server): State<Arc<VeldMapServerImpl>>,
) -> impl IntoResponse {
    let id = TileId { z: z as i32, x: x as i32, y: y as i32 };
    let path = server.find_local_path(id, "dem", "tif");
    
    let result = if path.exists() {
        println!("Serving tile: {:?}", path);
        server.load_tiff(Some(id), &path)
    } else {
        let dem_dir = server.config.data_path.join("dem");
        let fallback = std::fs::read_dir(&dem_dir).ok()
            .and_then(|mut entries| entries.find_map(|e| {
                let p = e.ok()?.path();
                if p.extension()?.to_str()? == "tif" { Some(p) } else { None }
            }));

        if let Some(p) = fallback {
            println!("Tile {}/{}/{} not found, using fallback: {:?}", z, x, y, p);
            server.load_tiff(Some(id), &p)
        } else {
            Err("No terrain data available".to_string())
        }
    };

    match result {
        Ok(tile) => (StatusCode::OK, Json(DemTileResponse { heights: tile.heights.clone(), width: tile.width, height: tile.height })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

async fn get_geoid(
    State(server): State<Arc<VeldMapServerImpl>>,
) -> impl IntoResponse {
    let geoid_dir = server.config.data_path.join("geoids");
    let geoid_path = std::fs::read_dir(&geoid_dir).ok()
        .and_then(|mut entries| entries.find_map(|e| {
            let p = e.ok()?.path();
            if p.extension()?.to_str()? == "pgm" { Some(p) } else { None }
        }));

    let result = if let Some(path) = geoid_path {
        println!("Serving geoid: {:?}", path);
        server.load_pgm(&path)
    } else {
        Err("Geoid not found".to_string())
    };

    match result {
        Ok(tile) => (StatusCode::OK, Json(DemTileResponse { heights: tile.heights.clone(), width: tile.width, height: tile.height })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}