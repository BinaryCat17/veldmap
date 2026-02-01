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
use serde::Deserialize;
use crate::app::{VeldMapToolsGui, Message};
use veldmap_iced_wasm_runtime::IcedRuntime;

// Контейнер для типов, которые не реализуют Send/Sync.
pub(crate) struct UnsafeSync<T>(pub T);
unsafe impl<T> Sync for UnsafeSync<T> {}
unsafe impl<T> Send for UnsafeSync<T> {}

#[derive(Deserialize)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalState(pub(crate) UnsafeSync<IcedRuntime<Message, VeldMapToolsGui>>);

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: handlers::module_init,
    handlers: {
        "handle_ui_event" => handlers::handle_ui_event : UiEvent => RpcResponse,
        "render" => handlers::handle_render : Empty => RpcResponse,
    }
}
