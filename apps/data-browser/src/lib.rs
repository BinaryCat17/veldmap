mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;
mod handlers;

use veldmap_rust_rpc::define_module;
use veldmap_rust_rpc::ui::UiEvent;
use veldmap_rust_rpc::services::RpcResponse;
use veldmap_rust_rpc::common::Empty;
use iced_core::Point;
use serde::Deserialize;
use crate::app::VeldMapToolsGui;
use iced_tiny_skia::Renderer;
use iced_runtime::user_interface;
use std::cell::RefCell;

// Контейнер для типов, которые не реализуют Send/Sync.
// В WASM это безопасно, так как поток всегда один.
pub(crate) struct UnsafeSync<T>(pub T);
unsafe impl<T> Sync for UnsafeSync<T> {}
unsafe impl<T> Send for UnsafeSync<T> {}

#[derive(Deserialize)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalStateInner {
    pub(crate) gui: RefCell<VeldMapToolsGui>,
    pub(crate) canvas_size: RefCell<(u32, u32)>,
    pub(crate) scale_factor: RefCell<f32>,
    pub(crate) cursor_position: RefCell<Point>,
    pub(crate) pending_events: RefCell<Vec<iced_core::Event>>,
    pub(crate) renderer: RefCell<Renderer>,
    pub(crate) interface_cache: RefCell<user_interface::Cache>,
    pub(crate) fonts_loaded: RefCell<bool>,
}

pub(crate) struct LocalState(pub(crate) UnsafeSync<LocalStateInner>);

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: handlers::module_init,
    handlers: {
        "handle_ui_event" => handlers::handle_ui_event : UiEvent => RpcResponse,
        "render" => handlers::handle_render : Empty => RpcResponse,
    }
}