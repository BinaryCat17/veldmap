pub mod rpc;
pub mod core;

#[cfg(feature = "wgpu")]
pub mod wgpu;
#[cfg(feature = "app")]
pub mod app;

pub use core::{yield_now, FLAG_PERF};

// Re-exports for macros
pub use serde_json;
pub use prost;
pub use anyhow;
pub use paste;
pub use futures_util;

pub use rpc::core::ResourceHandle;
use rpc::host::call_service;
use prost::Message;

pub const SURFACE_ID: u64 = 0;

pub struct OwnedResource {
    handle: ResourceHandle,
}

impl OwnedResource {
    pub fn new(handle: ResourceHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> ResourceHandle {
        self.handle.clone()
    }

    pub fn id(&self) -> u64 {
        self.handle.id
    }

    pub fn leak(self) -> ResourceHandle {
        let handle = self.handle.clone();
        std::mem::forget(self);
        handle
    }
}

impl Drop for OwnedResource {
    fn drop(&mut self) {
        let req = rpc::core::ReleaseResourceRequest { id: self.handle.id };
        let _ = call_service("system", "release_resource", req.encode_to_vec());
    }
}

impl Clone for OwnedResource {
    fn clone(&self) -> Self {
        let req = rpc::core::AcquireResourceRequest { id: self.handle.id };
        let _ = call_service("system", "acquire_resource", req.encode_to_vec());
        Self { handle: self.handle.clone() }
    }
}

impl AsRef<ResourceHandle> for OwnedResource {
    fn as_ref(&self) -> &ResourceHandle {
        &self.handle
    }
}

pub mod prelude {
    pub use crate::rpc::core::*;
    pub use crate::OwnedResource;
    #[cfg(feature = "pdk")]
    pub use crate::core::{Command, BoxedStream};
}
