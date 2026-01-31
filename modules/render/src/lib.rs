mod engine;
mod camera;
mod tiling;

use std::sync::Arc;
use veldmap_core::render_module::Renderer;
use crate::engine::State;

#[derive(Debug, Clone, Copy)]
pub enum RenderBackend {
    Vulkan,
    Metal,
    Dx12,
    Gl,
    BrowserWebGpu,
}

pub struct RenderConfig {
    pub backend: RenderBackend,
}

/// Единственная публичная точка входа в модуль рендеринга.
pub async fn create_renderer<W>(window: W, config: RenderConfig) -> Arc<dyn Renderer>
where
    W: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle + Send + Sync + 'static,
{
    let state = State::new(window, config).await;
    Arc::new(state)
}
