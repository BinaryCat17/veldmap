use veld_ui::proto::*;
use crate::handlers::*;
use veldsdk::define_module;
use crate::state::LocalState;

mod state;
mod handlers;
mod renderer;
mod converter;
mod graphics;

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

fn init_module(_config: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState::new())
}

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: init_module,
    handlers: {
        "ui-service/set_view" => handle_set_view,
        "host/handle_ui_event" => handle_ui_event,
        "host/frame" => handle_frame,
    }
}