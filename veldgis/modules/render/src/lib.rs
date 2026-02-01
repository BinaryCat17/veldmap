mod render;

use veldsdk::define_module;
use veldmap_gis_api::render::*;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalState;

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: render::module_init,
    handlers: {
        "render_frame" => render::handle_render_frame : RenderFrameRequest => RenderFrameResponse,
        "update_camera" => render::handle_update_camera : UpdateCameraRequest => UpdateCameraResponse,
        "upload_tile" => render::handle_upload_tile : UploadTileRequest => UploadTileResponse,
    }
}