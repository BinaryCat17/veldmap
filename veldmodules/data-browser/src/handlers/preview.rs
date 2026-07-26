//! Экран предпросмотра: открыть файл ресурсом, отдать его image-loader,
//! показать текстуру.
//!
//! Ресурс открываем мы, а не загрузчик: он не должен знать, лежит файл на
//! диске или на той стороне сети — читаются они одинаково. Мы же его и
//! закрываем, когда декодирование кончилось.
//!
//! Формат не проверяем: его определяет по содержимому image-loader, и второй
//! список расширений здесь был бы вторым источником правды (и разошёлся бы
//! с первым). Нераспознанный файл вернётся ошибкой и покажется на экране.

use crate::module::state::{State, Screen};
use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};
use crate::proto::ui_service::proto::UiEventResponse;
use veldsdk::proto::fs::{FsReadRequest, FsReadResult};
use veldsdk::proto::tasks::TaskCancelRequest;

/// Бросает текущее превью: освобождает ресурсы и отменяет декодирование, если
/// оно ещё идёт. Снимок декодируется секундами, и продолжать работу ради
/// картинки, которую уже никто не увидит, незачем.
pub fn abandon(state: &mut State) {
    if let Some(task_id) = state.preview.reset() {
        crate::calls::tasks::on_cancel(&TaskCancelRequest { task_id });
    }
}

/// Просмотр скачанного файла: открываем его через fs.
pub fn on_view_pressed(state: &mut State, event: UiEventResponse) {
    let path = event.value;
    if path.is_empty() { return; }

    // Один correlation_id на оба шага: по нему же отменяется декодирование.
    let correlation_id = open_preview(state, path.clone());
    crate::calls::fs::on_read(&FsReadRequest { path, correlation_id });
}

/// Просмотр удалённого файла — без скачивания. Ресурс открывает data-provider
/// (подписать запрос к хранилищу может только он) и передаёт нам владение;
/// дальше путь тот же, что у локального файла: read-грант загрузчику и декод
/// по фрагментам — по проводу идёт только то, что декодер действительно
/// прочитал.
pub fn on_preview_pressed(state: &mut State, event: UiEventResponse) {
    let identifier = event.value;
    if identifier.is_empty() { return; }

    let correlation_id = open_preview(state, identifier.clone());
    crate::calls::data_provider::on_preview(&crate::proto::data_provider::PreviewRequest {
        identifier,
        correlation_id,
    });
}

/// Общее начало обоих путей: экран превью, отказ от предыдущего, новый id.
fn open_preview(state: &mut State, label: String) -> String {
    state.current_screen = Screen::Preview;
    abandon(state);
    state.preview.current_path = label;

    let correlation_id = veldsdk::generate_id();
    state.preview.inflight = Some(correlation_id.clone());
    correlation_id
}

/// data-provider открыл удалённый ресурс — дальше как с локальным файлом.
pub fn on_preview_result(state: &mut State, response: crate::proto::data_provider::PreviewResponse) {
    if state.preview.inflight.as_deref() != Some(response.correlation_id.as_str()) {
        // Ответ на брошенный запрос: ресурс уже наш, освобождаем на месте.
        if let Some(resource) = response.resource {
            veldsdk::abi::arena_free(resource.id);
        }
        return;
    }
    match response.resource {
        Some(resource) => start_decode(state, resource, response.correlation_id),
        None => {
            state.preview.inflight = None;
            state.preview.error = Some(if response.error.is_empty() {
                "data-provider вернул пустой ресурс".to_string()
            } else {
                response.error
            });
        }
    }
}

/// fs открыл файл. Топик общий с чтением сидкаров (см. handlers::download),
/// поэтому свой ответ узнаём по correlation_id: false — ответ не наш.
pub fn on_file_opened(state: &mut State, resp: &FsReadResult) -> bool {
    if state.preview.inflight.as_deref() != Some(resp.correlation_id.as_str()) {
        return false;
    }
    match resp.handle.clone() {
        Some(handle) => start_decode(state, handle, resp.correlation_id.clone()),
        None => {
            state.preview.inflight = None;
            state.preview.error = Some(if resp.error.is_empty() {
                "fs вернул пустой handle".to_string()
            } else {
                resp.error.clone()
            });
        }
    }
    true
}

/// Ресурс открыт (файл или удалённый — читаются они одинаково): отдаём его
/// загрузчику. Владение остаётся у нас, поэтому и закрываем его мы — после
/// ответа, каким бы он ни был.
fn start_decode(state: &mut State, resource: veldsdk::ResourceHandle, correlation_id: String) {
    state.preview.file = Some(resource.id);
    if !veldsdk::abi::arena_grant_read(resource.id, "image-loader") {
        state.preview.inflight = None;
        state.preview.close_file();
        state.preview.error = Some("Не удалось выдать image-loader read-грант на ресурс".to_string());
        return;
    }

    // Бокс превью — размер окна в физических пикселях: больше на экран всё
    // равно не поместится, а декодировать в полный размер снимка незачем.
    crate::calls::image_loader::on_load(&LoadImageRequest {
        resource: Some(resource),
        max_width: state.window.0,
        max_height: state.window.1,
        correlation_id,
    });
}

/// Ответ image-loader. Broadcast — сверяем correlation_id, а заодно и то, что
/// запрос ещё актуален: пока ответ шёл, пользователь мог уйти с экрана или
/// открыть другой файл. Текстуру такого ответа освобождаем на месте —
/// владение уже передано нам, и потерять её значит потерять видеопамять.
pub fn on_load_result(state: &mut State, result: LoadImageResult) {
    if state.preview.inflight.as_deref() != Some(result.correlation_id.as_str()) {
        if let Some(handle) = result.handle {
            veldsdk::abi::arena_free(handle.id);
        }
        return;
    }
    state.preview.inflight = None;
    // Файл больше не нужен: декодирование кончилось (успехом или нет).
    state.preview.close_file();

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
