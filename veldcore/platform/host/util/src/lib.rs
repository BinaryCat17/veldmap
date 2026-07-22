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
pub use veldmap_host_core::dispatcher::{AsyncNativeService, Dispatcher};

// ── Контекст хоста ──────────────────────────────────────────────────────────
// Ручка на собранную платформу: dispatcher, memory, registry.
// Передаётся в register_services() каждого модуля.
pub use veldmap_host_core::setup::HostContext;

// ── Система фоновых задач ───────────────────────────────────────────────────
// Фасад над реестром задач: spawn с lifecycle-событиями, отмена и
// делегирование с проверкой lease. Модули собирают свой Tasks в init().
mod tasks;
pub use tasks::Tasks;

// ── Права доступа к ресурсам ────────────────────────────────────────────────
pub use veldmap_host_core::registry::Access;

// ── Протокол ────────────────────────────────────────────────────────────────
// Protobuf-типы шины (veldmap.core): запросы/ответы/события.
pub use veldmap_host_core::core;

// ── Сгенерированные биндинги платформенных контрактов ───────────────────────
// proto-типы и стабы топиков (topics::*/emit::*) из platform/host/generated
// (buildgen, veldcore/proto/*.schema.yaml). Публикация выходов сервиса —
// только через emit-стабы: bindings::fs::emit::on_read_result(&dispatcher, &msg).
pub use veldmap_host_bindings as bindings;

// ── Хелперы ─────────────────────────────────────────────────────────────────
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

    /// Резолвит путь из запроса модуля: относительный — от project_root
    /// хоста (каталог runtime/), абсолютный возвращается как есть.
    pub fn resolve_path(ctx: &crate::HostContext, path: &str) -> std::path::PathBuf {
        let path_obj = std::path::Path::new(path);
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { ctx.config.project_root.join(path_obj) }
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
                log::error!(target: "host", "Failed to decode {}: {}", topic, e);
                None
            }
        }
    }

    /// Подписывает один асинхронный сервис сразу на несколько его топиков.
    /// События придут последовательно, в порядке публикации (актор-очередь
    /// в диспетчере — одна на сервис).
    pub fn subscribe_async<S>(ctx: &HostContext, service: &Arc<S>, topics: &[&str])
    where
        S: AsyncNativeService + 'static,
    {
        ctx.dispatcher.subscribe(service.clone(), topics);
    }
}
