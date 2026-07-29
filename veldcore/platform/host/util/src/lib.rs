//! veldmap-host-util — публичный API для авторов нативных модулей хоста.
//!
//! Это фасад над `veldmap-host-core`: крейт-владелец реализации остаётся в core,
//! а здесь — курируемый список того, что модулям разрешено использовать,
//! плюс собственные хелперы. Нативные модули (host/modules/*) зависят ТОЛЬКО
//! от этого крейта и не импортируют `veldmap-host-core` напрямую.
//!
//! Правило наполнения: сюда попадает только то, что понадобилось минимум
//! одному модулю. Внутренности платформы (загрузчик плагинов, setup, stats)
//! сюда не re-export'ируются никогда.

// ── Трейты сервисов ─────────────────────────────────────────────────────────
// Контракт, который реализует каждый нативный модуль.
pub use veldmap_host_core::dispatcher::{AsyncNativeService, Caller, Dispatcher};

// ── Контекст хоста ──────────────────────────────────────────────────────────
// Ручка на собранную платформу: dispatcher, memory, registry.
// Передаётся в register_services() каждого модуля.
pub use veldmap_host_core::setup::HostContext;

// ── Система фоновых задач ───────────────────────────────────────────────────
// Фасад над реестром задач: spawn с lifecycle-событиями, отмена и
// делегирование с проверкой lease. Модули собирают свой Tasks в init().
mod tasks;
pub use tasks::Tasks;

// ── Поверхности окон ────────────────────────────────────────────────────────
// Фасад над очередью поверхностей: аттач с проверкой владения окном и
// write-lease на текстуру. Реализация контракта app — модуль host/modules/app.
mod surfaces;
pub use surfaces::Surfaces;

// ── Права доступа к ресурсам ────────────────────────────────────────────────
pub use veldmap_host_core::registry::Access;

// ── Носители ресурсов ───────────────────────────────────────────────────────
// Модуль, знающий свой протокол, реализует RangeSource и отдаёт его
// memory.alloc_range(). Файл на диске подключается тем же способом, так что
// для читателя диск и сеть — один и тот же ресурс.
pub use veldmap_host_core::memory::RangeSource;

// ── Протокол ────────────────────────────────────────────────────────────────
// Protobuf-типы шины (veldmap.core): запросы/ответы/события.
pub use veldmap_host_core::core;

// ── Сгенерированные биндинги платформенных контрактов ───────────────────────
// proto-типы и стабы топиков (topics::*/emit::*) из platform/host/generated
// (buildgen, veldcore/interface *.schema.yaml). Публикация выходов сервиса —
// только через emit-стабы: bindings::fs::emit::on_read_result(&dispatcher, &msg).
pub use veldmap_host_bindings as bindings;

// ── Хелперы ─────────────────────────────────────────────────────────────────

/// Выполняет тело обработчика на blocking-пуле.
///
/// Обработчики нативных модулей вызываются диспетчером внутри async-задачи,
/// поэтому обращение к диску (или к любому другому медленному носителю)
/// прямо в них занимает воркер рантайма целиком: встанет не только этот
/// сервис, а всё, что живёт на том же потоке. Ответное событие публикуется
/// из тела — dispatcher для этого достаточно послать сообщение в канал.
///
/// Цена: сервис перестаёт быть последовательным актором (см. Dispatcher::
/// subscribe) — вынесенные тела идут параллельно и отвечают в любом порядке.
/// Поэтому выносить можно только то, что корреспондент опознаёт по
/// correlation_id, и нельзя — операции, чей порядок между собой значим.
pub fn blocking<F>(ctx: &std::sync::Arc<HostContext>, job: F)
where
    F: FnOnce(std::sync::Arc<HostContext>) + Send + 'static,
{
    let ctx = ctx.clone();
    tokio::task::spawn_blocking(move || job(ctx));
}

pub mod path {
    /// Проверяет, что относительный путь не выходит за пределы рабочей
    /// директории: запрещает абсолютные пути и компоненты `..`.
    pub fn is_path_safe(path: &str) -> bool {
        let path_obj = std::path::Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }

    /// Резолвит путь из запроса модуля: относительный — от runtime_dir
    /// хоста (каталог runtime/), абсолютный возвращается как есть.
    pub fn resolve_path(ctx: &crate::HostContext, path: &str) -> std::path::PathBuf {
        let path_obj = std::path::Path::new(path);
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { ctx.config.runtime_dir.join(path_obj) }
    }
}

/// Сантехника protobuf-шины: единое место для decode и регистрации,
/// чтобы обработчики модулей содержали только бизнес-логику.
/// Публикация — через сгенерированные emit-стабы (crate::bindings).
pub mod wire {
    use super::{AsyncNativeService, HostContext};
    use prost::Message;
    use std::sync::Arc;

    /// Декодирует protobuf-payload события. При ошибке логирует и возвращает
    /// None — обработчику остаётся `let Some(req) = ... else { return };`.
    pub fn decode_or_log<T: Message + Default>(payload: &[u8], topic: &str) -> Option<T> {
        match T::decode(payload) {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!(target: "bus", "Failed to decode {}: {}", topic, e);
                None
            }
        }
    }

    /// Подписывает один асинхронный сервис сразу на несколько его топиков под
    /// его именем — оно делает сервис адресуемым для targeted-публикаций.
    /// События придут последовательно, в порядке публикации (актор-очередь
    /// в диспетчере — одна на сервис).
    pub fn subscribe_async<S>(ctx: &HostContext, name: &str, service: &Arc<S>, topics: &[&str])
    where
        S: AsyncNativeService + 'static,
    {
        ctx.dispatcher.subscribe_named(service.clone(), name, topics);
    }
}
