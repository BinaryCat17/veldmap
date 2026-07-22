pub use crate::module::state::State;

mod state;
mod handlers;
mod renderer;
mod converter;
mod graphics;
mod keyboard;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

// -- Init --
pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State::new())
}

// -- Input handlers --
pub use handlers::{handle_set_view as on_set_view, handle_set_surface as on_set_surface};

// -- Platform subscriptions --
pub use handlers::handle_ui_event as on_ui_event;