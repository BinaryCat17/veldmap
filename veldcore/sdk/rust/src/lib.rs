pub mod abi;
pub mod proto;
pub mod runtime;
pub mod logging;
pub mod graphics;
pub mod app;
pub mod tasks;
pub mod tracker;
pub mod correlator;

pub use logging::FLAG_PERF;

pub const FLAG_SDK: u32 = 1 << 6;
pub const FLAG_UI_SERVICE: u32 = 1 << 7;
pub const FLAG_UI_HANDLERS: u32 = 1 << 8;
pub const FLAG_GRAPHICS: u32 = 1 << 9;

pub use serde_json;
pub use prost;
pub use anyhow;
pub use log;

pub use proto::core::ResourceHandle;
pub use abi::generate_id;
pub use abi::event_publisher;
pub use tracker::TaskTracker;
pub use correlator::Correlator;

/// RAII handle to a memory region or graphics object.
/// On drop: releases the resource via memory ABI.
/// Deliberately not Clone: exactly one owner frees the resource.
pub struct OwnedResource {
    handle: ResourceHandle,
}

impl OwnedResource {
    pub fn new(handle: ResourceHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> ResourceHandle { self.handle.clone() }
    pub fn id(&self) -> u64 { self.handle.id }
}

impl Drop for OwnedResource {
    fn drop(&mut self) {
        abi::arena_free(self.handle.id);
    }
}

impl AsRef<ResourceHandle> for OwnedResource {
    fn as_ref(&self) -> &ResourceHandle { &self.handle }
}

pub mod prelude {
    pub use crate::proto::core::*;
    pub use crate::OwnedResource;
}
