use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, BufReader, BufRead};
use tiff::decoder::{Decoder, DecodingResult};
use veldmap_core::data_module::{TileId, DemTile};
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
    let id = TileId { z, x, y };
    let path = server.find_local_path(id, "dem", "tif");
    
    let result = if path.exists() {
        server.load_tiff(&path)
    } else {
        let rostov = server.config.data_path.join("dem").join("rostov_tile.tif");
        if rostov.exists() {
            server.load_tiff(&rostov)
        } else {
            Err("Tile not found".to_string())
        }
    };

    match result {
        Ok(tile) => (StatusCode::OK, Json(DemTileResponse { heights: tile.heights, width: tile.width, height: tile.height })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

async fn get_geoid(
    State(server): State<Arc<VeldMapServerImpl>>,
) -> impl IntoResponse {
    let path = server.config.data_path.join("geoids/egm2008-5.pgm");
    match server.load_pgm(&path) {
        Ok(tile) => (StatusCode::OK, Json(DemTileResponse { heights: tile.heights, width: tile.width, height: tile.height })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}
