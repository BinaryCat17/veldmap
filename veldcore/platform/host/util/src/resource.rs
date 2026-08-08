//! Сборка ответа «ресурс открыт» — зеркало `veldsdk::resource::opened` для
//! нативной стороны.
//!
//! Открытие ресурса отвечает одним и тем же `core.ResourceOpened` у fs,
//! network и прикладных модулей, и раскладка «успех → handle, отказ → текст»
//! у всех одна. Собранная руками, она расходится в мелочах: пустая строка
//! против отсутствующего поля, handle рядом с непустой ошибкой.
//!
//! Публикует ответ модуль сам, своим emit-стабом: топики объявлены в его
//! схеме, и util о них не знает.

use veldmap_host_core::core::{ResourceHandle, ResourceOpened};

pub fn opened(result: Result<ResourceHandle, String>) -> ResourceOpened {
    let (handle, error) = match result {
        Ok(handle) => (Some(handle), String::new()),
        Err(error) => (None, error),
    };
    ResourceOpened { handle, error }
}

/// То же по идентификатору и размеру — обычная форма на стороне хоста, где
/// открытие возвращает `(id, len)`.
pub fn opened_handle(id: u64, size: u64) -> ResourceOpened {
    opened(Ok(ResourceHandle { id, size }))
}
