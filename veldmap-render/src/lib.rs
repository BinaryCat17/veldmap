mod engine;
mod camera;
pub mod tiling;

use std::sync::Arc;
use veldmap_core::render_module::Renderer;
pub use crate::engine::State;

/// Единственная публичная точка входа в модуль рендеринга.
pub async fn create_renderer<W>(window: W) -> Arc<dyn Renderer>
where
    W: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle + Send + Sync + 'static,
{
    let state = State::new(window).await;
    Arc::new(state)
}