pub mod rpc;
pub mod core;

#[cfg(feature = "compute")]
pub mod compute;
#[cfg(feature = "app")]
pub mod app;

pub use core::FLAG_PERF;

// Flags for logging
pub const FLAG_SDK: u32 = 1 << 6;
pub const FLAG_UI_SERVICE: u32 = 1 << 7;
pub const FLAG_UI_HANDLERS: u32 = 1 << 8;
pub const FLAG_GRAPHICS: u32 = 1 << 9;

// Re-exports for macros
pub use serde_json;
pub use prost;
pub use anyhow;
pub use paste;
pub use log;

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
}
