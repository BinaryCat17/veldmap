//! Iced integration for VeldMap WASM plugins.

use iced_core::{Font, Theme};
use iced_tiny_skia::Renderer;
use serde::Deserialize;

pub mod runtime;

/// Settings for initializing the Iced runtime.
pub struct IcedSettings {
    pub default_font: Font,
    pub fonts: Vec<(&'static str, &'static [u8])>,
}

/// The unified trait for building Iced-based VeldMap modules.
/// 
/// This trait combines state management, initialization, and UI rendering.
pub trait IcedModule: Sized {
    /// The type of messages your application handles.
    type Message: Send + 'static;
    
    /// The configuration type for your module.
    type Config: for<'de> Deserialize<'de>;

    /// Initializes the module with the given configuration.
    /// Returns the module instance and UI settings.
    fn init(config: Self::Config) -> anyhow::Result<(Self, IcedSettings)>;

    /// Handles updates to the module state based on messages.
    fn update(&mut self, message: Self::Message);
    
    /// Returns the view for the current state of the module.
    fn view(&self) -> iced_core::Element<'_, Self::Message, Theme, Renderer>;

    /// Optional: Decodes an RPC call into an Iced message.
    /// This allows your UI to react to external RPC calls.
    fn decode_rpc(_method: &str, _payload: &[u8]) -> anyhow::Result<Option<Self::Message>> {
        Ok(None)
    }
}

/// Internal trait used by the macro to drive the UI without knowing the Message type.
pub trait RawIcedRuntime: Send + Sync {
    fn handle_event(&self, event: crate::rpc::ui::UiEvent) -> anyhow::Result<()>;
    fn render(&self) -> anyhow::Result<()>;
}
