use crate::module::handlers::*;
pub use crate::module::state::LocalState;

mod state;
mod handlers;
mod renderer;
mod converter;
mod graphics;

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

// -- Types --
pub use LocalConfig as Config;
pub use LocalState as State;

// -- Init --
pub fn init_module(_config: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState::new())
}

// -- Input handlers --
pub use handlers::{handle_set_view as on_input_set_view, handle_ui_event as on_input_handle_ui_event};