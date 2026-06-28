pub mod rpc;
pub mod core;

#[cfg(feature = "compute")]
pub mod compute;
#[cfg(feature = "app")]
pub mod app;

pub use core::FLAG_PERF;

pub const FLAG_SDK: u32 = 1 << 6;
pub const FLAG_UI_SERVICE: u32 = 1 << 7;
pub const FLAG_UI_HANDLERS: u32 = 1 << 8;
pub const FLAG_GRAPHICS: u32 = 1 << 9;

pub use serde_json;
pub use prost;
pub use anyhow;
pub use paste;
pub use log;

pub use rpc::core::ResourceHandle;
pub const SURFACE_ID: u64 = 0;

/// RAII handle to a memory region or graphics object.
/// On drop: releases the resource via memory ABI (no RPC overhead).
/// On clone: acquires a read lease via memory ABI.
pub struct OwnedResource {
    handle: ResourceHandle,
}

impl OwnedResource {
    pub fn new(handle: ResourceHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> ResourceHandle { self.handle.clone() }
    pub fn id(&self) -> u64 { self.handle.id }

    pub fn leak(self) -> ResourceHandle {
        let handle = self.handle.clone();
        std::mem::forget(self);
        handle
    }
}

impl Drop for OwnedResource {
    fn drop(&mut self) {
        #[cfg(feature = "pdk")]
        {
            rpc::host::arena_free(self.handle.id);
        }
    }
}

impl Clone for OwnedResource {
    fn clone(&self) -> Self {
        Self { handle: self.handle.clone() }
    }
}

impl AsRef<ResourceHandle> for OwnedResource {
    fn as_ref(&self) -> &ResourceHandle { &self.handle }
}

pub mod prelude {
    pub use crate::rpc::core::*;
    pub use crate::OwnedResource;
}
