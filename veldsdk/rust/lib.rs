pub mod core;
pub mod rpc;
#[cfg(feature = "wgpu")]
pub mod wgpu;
#[cfg(feature = "app")]
pub mod app;
#[cfg(feature = "iced")]
pub mod iced;

pub use core::yield_now;

// Re-exports for macros
pub use serde_json;
pub use prost;
pub use anyhow;

pub mod prelude {
    pub use crate::rpc::core::*;
    #[cfg(feature = "pdk")]
    pub use crate::core::*;
    #[cfg(feature = "wgpu")]
    pub use crate::wgpu::*;
    #[cfg(feature = "app")]
    pub use crate::app::*;
    #[cfg(feature = "iced")]
    pub use crate::iced::runtime::GpuRenderer;
}
    