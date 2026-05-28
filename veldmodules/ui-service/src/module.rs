pub use crate::module::state::State;

mod state;
mod handlers;
mod renderer;
mod converter;
mod graphics;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

// -- Init --
pub fn init(_config: Config) -> anyhow::Result<State> {
    Ok(State::new())
}

// -- Input handlers --
pub use handlers::{handle_set_view as on_input_set_view, handle_ui_event as on_input_handle_ui_event};