//! Типизированные стабы платформенных топиков `app/*` (контракт
//! veldcore/proto/app.proto). Строка топика существует только здесь,
//! модули зовут эти функции вместо сырого `abi::publish`.

use prost::Message;
use crate::proto::app as proto;
use crate::proto::core::ResourceHandle;

/// Приаттачить render-target к окну модуля (`app/set_surface`).
/// Вызывается владельцем окна после выделения текстуры; хост начнёт
/// композить её в окно plugin_id. Свап атомарный.
pub fn set_surface(plugin_id: &str, surface: ResourceHandle) {
    crate::abi::publish("app/set_surface", proto::SetSurface {
        plugin_id: plugin_id.to_string(),
        surface: Some(surface),
    }.encode_to_vec());
}
