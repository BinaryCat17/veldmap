mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;
mod handlers;

use veldsdk::define_module;
use veldsdk::rpc::ui::UiEvent;
use veldsdk::rpc::services::RpcResponse;
use veldmap_gis_api::common::Empty;
use serde::Deserialize;
use crate::app::{VeldMapToolsGui, Message};
use veldsdk::iced::IcedRuntime;

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
