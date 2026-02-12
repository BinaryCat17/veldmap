pub mod rpc;
pub mod core;

#[cfg(feature = "wgpu")]
pub mod wgpu;
#[cfg(feature = "app")]
pub mod app;

pub use core::yield_now;

// Re-exports for macros
pub use serde_json;
pub use prost;
pub use anyhow;
pub use paste;
pub use futures_util;

pub mod prelude {
    pub use crate::rpc::core::*;
    #[cfg(feature = "pdk")]
    pub use crate::core::{Command, BoxedFuture};
}