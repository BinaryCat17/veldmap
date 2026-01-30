use crate::common_module::{TileId, DemTile};
use std::sync::Arc;

#[uniffi::export(callback_interface)]
pub trait Renderer: Send + Sync {
    fn render(&self) -> Result<(), String>;
    fn resize(&self, width: u32, height: u32);
    fn update(&self);
    fn upload_tile(&self, id: TileId, dem: Arc<DemTile>);
    fn set_geoid(&self, dem: Arc<DemTile>);
    fn camera_zoom(&self, delta: f64);
    fn camera_move(&self, dx: f64, dy: f64);
}
