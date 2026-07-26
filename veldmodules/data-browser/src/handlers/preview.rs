//! Экран предпросмотра: запрос превью у image-loader и приём ответа.
//!
//! Формат мы не проверяем: его определяет по содержимому сам image-loader, и
//! второй список расширений тут был бы вторым источником правды (и разошёлся
//! бы с первым). Нераспознанный файл вернётся ошибкой и покажется на экране.

use crate::module::state::{State, Screen};
use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};
use crate::proto::ui_service::proto::UiEventResponse;

pub fn on_view_pressed(state: &mut State, event: UiEventResponse) {
    let path = event.value;
    if path.is_empty() { return; }

    state.current_screen = Screen::Preview;
    state.preview.reset();
    state.preview.current_path = path.clone();

    // Бокс превью — размер окна в физических пикселях: больше на экран всё
    // равно не поместится, а декодировать в полный размер снимка незачем.
    let correlation_id = veldsdk::generate_id();
    state.preview.inflight = Some(correlation_id.clone());
    crate::calls::image_loader::on_load(&LoadImageRequest {
        path,
        max_width: state.window.0,
        max_height: state.window.1,
        correlation_id,
    });
}

/// Ответ image-loader. Broadcast — сверяем correlation_id, а заодно и то,
/// что запрос ещё актуален: пока ответ шёл, пользователь мог уйти с экрана
/// или открыть другой файл. Текстуру такого ответа освобождаем на месте —
/// владение уже передано нам, и потерять её значит потерять видеопамять.
pub fn on_load_result(state: &mut State, result: LoadImageResult) {
    if state.preview.inflight.as_deref() != Some(result.correlation_id.as_str()) {
        if let Some(handle) = result.handle {
            veldsdk::abi::arena_free(handle.id);
        }
        return;
    }
    state.preview.inflight = None;

    if !result.error.is_empty() {
        state.preview.error = Some(result.error);
        return;
    }
    let Some(handle) = result.handle else {
        state.preview.error = Some("image-loader вернул пустой handle".to_string());
        return;
    };

    // ui-service строит view/bind group этой текстуры по read-гранту —
    // тот же ритуал, что grant_write оконной поверхности (surface.rs).
    if !veldsdk::abi::arena_grant_read(handle.id, "ui-service") {
        veldsdk::abi::arena_free(handle.id);
        state.preview.error = Some("Не удалось выдать ui-service read-грант на текстуру".to_string());
        return;
    }
    state.preview.texture = Some(handle.id);
}
