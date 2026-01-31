// mod engine;
// mod camera;
// mod tiling;

use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use veldmap_rust_rpc::render::*;
use prost::Message;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct LocalConfig {
    pub width: u32,
    pub height: u32,
}

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let _config: LocalConfig = match config::get("config") {
        Ok(Some(c)) => serde_json::from_str(&c)?,
        _ => LocalConfig { width: 800, height: 600 },
    };

    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "render_frame" => {
            let req = RenderFrameRequest::decode(&request.payload[..])?;
            // Возвращаем тестовый кадр (синий фон, RGBA)
            let mut image_data = vec![0u8; (req.width * req.height * 4) as usize];
            for chunk in image_data.chunks_exact_mut(4) {
                chunk[2] = 255; // Blue
                chunk[3] = 255; // Alpha
            }
            let frame = RenderFrameResponse {
                image_data,
                width: req.width,
                height: req.height,
                error: String::new(),
            };
            (frame.encode_to_vec(), String::new())
        }
        "update_camera" => (Vec::new(), String::new()),
        "upload_tile" => (Vec::new(), String::new()),
        _ => (Vec::new(), format!("Method {} not found in render", request.method)),
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}