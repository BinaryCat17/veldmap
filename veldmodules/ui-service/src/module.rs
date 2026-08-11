pub use crate::module::state::State;

mod state;
mod handlers;
mod renderer;
mod converter;
mod graphics;
mod keyboard;
mod popover;

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Формат поверхности окна (proto TextureFormat): инъектируется хостом
    /// в init-конфиг — знать его нужно до первого set_surface, пайплайн
    /// создаётся под него.
    #[serde(default)]
    pub surface_format: i32,
}

// -- Init --
pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State::new(config.surface_format))
}

// -- Input handlers --
pub use handlers::{handle_set_view as on_set_view, handle_set_surface as on_set_surface};

// -- Platform subscriptions --
pub use handlers::handle_ui_event as on_ui_event;
