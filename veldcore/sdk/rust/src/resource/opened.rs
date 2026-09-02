//! Сборка ответа «ресурс открыт» — один файл по обе стороны провода.
//!
//! Включают его SDK (`veldsdk::resource`) и `host/util` нативных fs и network
//! (через `#[path]`, как `abi/wire.rs`): раскладка «успех → handle, отказ →
//! текст» у всех отвечающих одна, а собранная руками, она расходилась бы в
//! мелочах — пустая строка против отсутствующего поля, handle рядом с
//! непустой ошибкой, — и по ним заказчик отличает «нет такого» от «не
//! отдам». Типы берёт у включившего: у обоих это `core.ResourceOpened` их
//! сгенерированного крейта.

use super::{ResourceHandle, ResourceOpened};

/// Собирает ответ на «открой мне это» — удача и неудача одной формы.
///
/// Публикует его модуль сам, своим стабом, и передаёт туда же корреляцию
/// запроса: топики объявлены в его схеме, и ни SDK, ни util о них не знают.
pub fn opened(result: Result<ResourceHandle, String>) -> ResourceOpened {
    let (handle, error) = match result {
        Ok(handle) => (Some(handle), String::new()),
        Err(error) => (None, error),
    };
    ResourceOpened { handle, error }
}
