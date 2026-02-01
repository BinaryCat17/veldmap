use veldmap_gis_api::render::*;
use crate::{LocalConfig, LocalState};

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState)
}

pub fn handle_render_frame(_state: &LocalState, req: RenderFrameRequest) -> anyhow::Result<RenderFrameResponse> {
    // Возвращаем тестовый кадр (синий фон, RGBA)
    let mut image_data = vec![0u8; (req.width * req.height * 4) as usize];
    for chunk in image_data.chunks_exact_mut(4) {
        chunk[2] = 255; // Blue
        chunk[3] = 255; // Alpha
    }
    Ok(RenderFrameResponse {
        image_data,
        width: req.width,
        height: req.height,
        error: String::new(),
    })
}

pub fn handle_update_camera(_state: &LocalState, _req: UpdateCameraRequest) -> anyhow::Result<UpdateCameraResponse> {
    Ok(UpdateCameraResponse { success: true, error: String::new() })
}

pub fn handle_upload_tile(_state: &LocalState, _req: UploadTileRequest) -> anyhow::Result<UploadTileResponse> {
    Ok(UploadTileResponse { success: true, error: String::new() })
}