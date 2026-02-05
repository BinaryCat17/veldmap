pub mod core;
pub mod rpc;
#[cfg(feature = "graphics")]
pub mod graphics;
#[cfg(feature = "iced")]
pub mod iced;

pub use core::yield_now;

// Re-exports for macros
pub use serde_json;
pub use prost;
pub use anyhow;

pub mod prelude {
    pub use crate::rpc::services::*;
    #[cfg(feature = "pdk")]
    pub use crate::core::*;
    #[cfg(feature = "graphics")]
    pub use crate::graphics::*;
        #[cfg(feature = "iced")]
        pub use crate::iced::runtime::GpuRenderer;
    }
    