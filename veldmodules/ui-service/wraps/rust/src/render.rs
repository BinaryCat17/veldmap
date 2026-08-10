//! Отправка layout модуля в ui-service (Elm-цикл view).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use veldsdk::prost::Message;

use super::widgets::Element;

/// Отправляет текущий view модуля рендереру.
///
/// Разметка едет целиком; неизменившаяся не едет вовсе — отсекается по хэшу
/// содержимого. Что и когда перерисовывать, решает уже рендерер.
///
/// `publish` — стаб топика `ui-service/on_set_view` из кодогена вызывающего
/// модуля: `render::render(id, root, &mut hash, crate::calls::ui_service::on_set_view)`.
/// Wrap-крейт не публикует сам: он один на всех потребителей и не знает, кто
/// из них объявил `ui-service: calls: [on_set_view]` у себя в schema.yaml, —
/// а публикация в обход объявления сделала бы граф связей в схемах неполным.
pub fn render<M>(
    plugin_id: &str,
    root: Element<M>,
    last_hash: &mut u64,
    publish: impl FnOnce(&crate::proto::SetViewRequest),
) {
    let layout = crate::proto::Layout { root: Some(root.widget) };

    let encoded = layout.encode_to_vec();
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    let hash = hasher.finish();

    if hash == *last_hash {
        return;
    }
    *last_hash = hash;

    let request = crate::proto::SetViewRequest {
        plugin_id: plugin_id.to_string(),
        layout: Some(layout),
    };
    publish(&request);
}
