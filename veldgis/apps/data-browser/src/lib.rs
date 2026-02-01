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
use veldmap_gis_api::common::Empty;
use serde::Deserialize;
use veldsdk::iced::UiRuntime;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalState(pub(crate) Box<dyn UiRuntime>);

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: handlers::module_init,
    handlers: {
        "handle_ui_event" => handlers::handle_ui_event : UiEvent => RpcResponse,
        "render" => handlers::handle_render : Empty => RpcResponse,
    }
}
