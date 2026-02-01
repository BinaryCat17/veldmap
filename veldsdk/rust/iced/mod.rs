//! Iced integration for VeldMap WASM plugins.
//!
//! This module provides the core traits and runtime needed to build
//! interactive GUIs using the Iced library within the VeldMap platform.

use iced_core::{Font, Theme};
use iced_tiny_skia::Renderer;

pub mod runtime;
pub use runtime::IcedRuntime;

/// The main trait for creating GUI applications.
/// Implement this trait for your state struct to define how the UI behaves.
pub trait Application<Message> {
    /// Handles updates to the application state based on messages.
    fn update(&mut self, message: Message);
    
    /// Returns the view for the current state of the application.
    fn view(&self) -> iced_core::Element<'_, Message, Theme, Renderer>;
}

/// Factory function to create a new Iced runtime.
///
/// This is the recommended way to initialize the UI in your plugin's `init` function.
pub fn create_runtime<M, A>(
    gui: A, 
    default_font: Font, 
    fonts: Vec<(&'static str, &'static [u8])>
) -> IcedRuntime<M, A> 
where 
    A: Application<M> 
{
    IcedRuntime::new(gui, default_font, fonts)
}
