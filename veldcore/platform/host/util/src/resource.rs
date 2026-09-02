//! Зеркало обряда «открой мне это» для нативной стороны. Сборка ответа — тот
//! же файл, что у SDK (`resource/opened.rs`, через `#[path]`); своя здесь
//! только форма, в которой хост держит открытое: `(id, len)`.
//!
//! Публикует ответ модуль сам, своим emit-стабом: топики объявлены в его
//! схеме, и util о них не знает.

use veldmap_host_core::core::{ResourceHandle, ResourceOpened};

#[path = "../../../../sdk/rust/src/resource/opened.rs"]
mod opened;
pub use opened::opened;

/// То же по идентификатору и размеру — обычная форма на стороне хоста, где
/// открытие возвращает `(id, len)`.
pub fn opened_handle(id: u64, size: u64) -> ResourceOpened {
    opened(Ok(ResourceHandle { id, size }))
}
