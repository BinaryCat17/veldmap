//! Iced integration for VeldMap WASM plugins.

use iced_core::Font;

pub mod runtime;

/// Settings for initializing the Iced runtime.
pub struct IcedSettings {
    pub default_font: Font,
    pub fonts: Vec<(&'static str, &'static [u8])>,
}

/// Internal trait used by the macro to drive the UI.
pub trait RawIcedRuntime: Send + Sync {
    fn handle_event(&self, event: crate::rpc::ui::UiEvent) -> anyhow::Result<()>;
    fn render(&self) -> anyhow::Result<()>;
}