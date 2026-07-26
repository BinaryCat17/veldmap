pub mod abi;
pub mod proto;
pub mod graphics;
pub mod tasks;
pub mod correlator;
pub mod resource;

/// Мост `log` → ABI хоста. Ставится сгенерированным клеем модуля; прикладной
/// код пишет обычными макросами крейта `log` (реэкспортирован ниже).
#[doc(hidden)]
pub mod logging;

/// Внутренности для сгенерированного клея модуля (buildgen, lib.rs.j2):
/// хранение состояния между вызовами хоста и диспетчеризация обработчиков.
/// Прикладной код сюда не обращается — модуль пишет только обработчики,
/// а вызывает их кодоген.
#[doc(hidden)]
pub mod runtime;

pub use serde_json;
pub use prost;
pub use anyhow;
pub use log;

pub use proto::core::ResourceHandle;
pub use abi::generate_id;
pub use abi::event_publisher;
pub use correlator::Correlator;
pub use tasks::{LocalTask, TaskTracker};
pub use resource::ResourceReader;

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
