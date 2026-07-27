//! Экран предпросмотра: открыть файл ресурсом, отдать его image-loader,
//! показать текстуру.
//!
//! Ресурс открываем мы, а не загрузчик: он не должен знать, лежит файл на
//! диске или на той стороне сети — читаются они одинаково. Мы же его и
//! закрываем, когда декодирование кончилось.
//!
//! Отсюда и один обработчик на оба источника: и fs, и data-provider отвечают
//! общим `core.ResourceOpened`, а дальше разницы между ними нет.
//!
//! Формат не проверяем: его определяет по содержимому image-loader, и второй
//! список расширений здесь был бы вторым источником правды (и разошёлся бы
//! с первым). Нераспознанный файл вернётся ошибкой и покажется на экране.

use crate::module::state::{State, Screen};
use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};
use crate::proto::ui_service::proto::UiEventResponse;
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::tasks::{TaskBeginRequest, TaskCancelRequest, TaskEndRequest};

/// Тип задачи декодирования в реестре платформы (идёт в tasks/task_started).
const DECODE_KIND: &str = "image_decode";

/// Бросает текущее превью: освобождает ресурсы и отменяет декодирование, если
/// оно ещё идёт. Снимок декодируется секундами, и продолжать работу ради
/// картинки, которую уже никто не увидит, незачем.
pub fn abandon(state: &mut State) {
    if let Some(task_id) = state.preview.reset() {
        crate::calls::tasks::on_cancel(&TaskCancelRequest { task_id });
    }
}

/// Просмотр скачанного файла: открывает библиотека — файл её, и где он лежит,
/// знает только она.
pub fn on_view_local_pressed(state: &mut State, event: UiEventResponse) {
    let name = event.value;
    if name.is_empty() { return; }

    // Один correlation_id на оба шага: по нему же отменяется декодирование.
    let correlation_id = begin_open(state, name.clone());
    crate::calls::data_library::on_open(&crate::proto::data_library::OpenRequest {
        name,
        correlation_id,
    });
}

/// Просмотр ещё не скачанного файла. Ресурс открывает data-provider (подписать
/// запрос к хранилищу может только он) и передаёт нам владение; дальше путь
/// тот же, что у локального файла: read-грант загрузчику и декод по фрагментам
/// — по проводу идёт только то, что декодер действительно прочитал.
pub fn on_view_remote_pressed(state: &mut State, event: UiEventResponse) {
    let identifier = event.value;
    if identifier.is_empty() { return; }

    let correlation_id = begin_open(state, identifier.clone());
    crate::calls::data_provider::on_open(&crate::proto::data_provider::OpenRequest {
        identifier,
        correlation_id,
    });
}

/// Общее начало обоих путей: экран превью, отказ от предыдущего, новый id.
fn begin_open(state: &mut State, label: String) -> String {
    state.current_screen = Screen::Preview;
    abandon(state);
    state.preview.current_path = label;

    let correlation_id = state.preview.opening.begin(());
    state.preview.inflight = Some(correlation_id.clone());
    correlation_id
}

/// Ресурс открыт — неважно кем: библиотекой (скачанный файл) или провайдером
/// (ещё не скачанный, читается по сети). Дальше разницы нет.
///
/// Устаревший ответ (пользователь успел уйти с экрана или открыть другое)
/// всё равно наш: ресурс уже принадлежит нам, и бросить его значит потерять
/// и регион, и открытый на той стороне дескриптор. `false` — ответ не наш.
pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    if !state.preview.opening.remove(&opened.correlation_id) {
        return false;
    }
    if state.preview.inflight.as_deref() != Some(opened.correlation_id.as_str()) {
        if let Some(handle) = &opened.handle {
            veldsdk::abi::arena_free(handle.id);
        }
        return true;
    }

    match opened.handle.clone() {
        Some(handle) => start_decode(state, handle, opened.correlation_id.clone()),
        None => {
            state.preview.inflight = None;
            state.preview.error = Some(if opened.error.is_empty() {
                "ресурс не открыт, но и ошибки не названо".to_string()
            } else {
                opened.error.clone()
            });
        }
    }
    true
}

/// Отдаём открытый ресурс загрузчику. Владение остаётся у нас, поэтому и
/// закрываем его мы — после ответа, каким бы он ни был.
fn start_decode(state: &mut State, resource: veldsdk::ResourceHandle, correlation_id: String) {
    state.preview.file = Some(resource.id);
    if !veldsdk::abi::arena_grant_read(resource.id, "image-loader") {
        state.preview.inflight = None;
        state.preview.close_file();
        state.preview.error = Some("Не удалось выдать image-loader read-грант на ресурс".to_string());
        return;
    }

    // Задачу заводим мы, а не загрузчик: владелец — паблишер begin, и только
    // он вправе её отменить. Загрузчик её лишь опрашивает, пока декодирует.
    state.preview.decode_task = Some(correlation_id.clone());
    crate::calls::tasks::on_begin(&TaskBeginRequest {
        task_id: correlation_id.clone(),
        kind: DECODE_KIND.to_string(),
        label: state.preview.current_path.clone(),
        executor: "image-loader".to_string(),
    });

    // Бокс превью — размер окна в физических пикселях: больше на экран всё
    // равно не поместится, а декодировать в полный размер снимка незачем.
    crate::calls::image_loader::on_load(&LoadImageRequest {
        resource: Some(resource),
        max_width: state.window.0,
        max_height: state.window.1,
        correlation_id,
        // Загрузчик получил безымянный ресурс — назвать источник в логах и в
        // списке задач можем только мы.
        label: state.preview.current_path.clone(),
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
    // Задачу заводили мы — нам её и закрывать, с тем же исходом, что у ответа.
    if let Some(task_id) = state.preview.decode_task.take() {
        crate::calls::tasks::on_end(&TaskEndRequest { task_id, error: result.error.clone() });
    }

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
