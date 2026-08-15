//! Отправка layout модуля в ui-service (Elm-цикл view).

use super::widgets::Element;

/// Отправляет текущий view модуля рендереру.
///
/// Разметка едет целиком; неизменившаяся не едет вовсе — топик `on_set_view`
/// объявлен снимком, и повтор отсекает его стаб (см. veldsdk::snapshot).
/// Что и когда перерисовывать, решает уже рендерер.
///
/// Себя называть не нужно: отправителя штампует хост, и ui-service читает его
/// из конверта (см. `SetViewRequest` в types.proto).
///
/// `publish` — стаб топика `ui-service/on_set_view` из кодогена вызывающего
/// модуля: `render::render(root, crate::calls::ui_service::on_set_view)`.
/// Wrap-крейт не публикует сам: он один на всех потребителей и не знает, кто
/// из них объявил `ui-service: calls: [on_set_view]` у себя в schema.yaml, —
/// а публикация в обход объявления сделала бы граф связей в схемах неполным.
pub fn render<M>(root: Element<M>, publish: impl FnOnce(&crate::proto::SetViewRequest)) {
    let layout = crate::proto::Layout { root: Some(root.widget) };
    publish(&crate::proto::SetViewRequest { layout: Some(layout) });
}
