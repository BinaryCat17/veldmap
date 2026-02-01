//! Iced integration for VeldMap WASM plugins.
//!
//! This module provides a clean interface for building interactive GUIs.

use iced_core::{Font, Theme, Point, Event};
use iced_tiny_skia::Renderer;

pub mod runtime;
pub use runtime::IcedRuntime;

/// The main trait for creating GUI applications.
/// 
/// Implement this trait for your state struct to define your UI logic.
pub trait Application {
    /// The type of messages your application will handle.
    type Message: Send + 'static;

    /// Handles updates to the application state based on messages.
    fn update(&mut self, message: Self::Message);
    
    /// Returns the view for the current state of the application.
    fn view(&self) -> iced_core::Element<'_, Self::Message, Theme, Renderer>;
}

/// A handle to the active UI runtime.
/// 
/// This trait hides the complex internal generics of the Iced implementation.
pub trait UiRuntime: Send + Sync {
    /// Updates the dimensions and scale factor of the UI canvas.
    fn update_size(&self, width: u32, height: u32, scale_factor: f32);
    
    /// Updates the absolute cursor position on the canvas.
    fn update_cursor(&self, x: f32, y: f32);
    
    /// Returns the current logical cursor position.
    fn cursor_position(&self) -> Point;
    
    /// Pushes a raw Iced event into the runtime queue.
    fn push_event(&self, event: Event);
    
    /// Renders the current frame and sends it to the host.
    fn render(&self) -> anyhow::Result<()>;
}

/// Creates a new UI runtime for the given application.
///
/// This is the entry point for UI-enabled plugins.
pub fn create_runtime<T: Application + 'static>(
    gui: T, 
    default_font: Font, 
    fonts: Vec<(&'static str, &'static [u8])>
) -> Box<dyn UiRuntime> {
    Box::new(IcedRuntime::new(gui, default_font, fonts))
}