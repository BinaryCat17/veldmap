//! Вид предпросмотра: открыть файл ресурсом, отдать его image-loader,
//! показать текстуру.
//!
//! Ресурс открываем мы, а не загрузчик: он не должен знать, лежит файл на
//! диске или на той стороне сети — читаются они одинаково. Мы же его и
//! закрываем, когда декодирование кончилось.
//!
//! Отсюда и один обработчик на оба источника: и data-library, и data-provider
//! отвечают общим `core.ResourceOpened`, а дальше разницы между ними нет.
//!
//! Каждый просмотр — своя вкладка со своим состоянием, поэтому «чей это
//! ответ» и «актуален ли он» — два разных вопроса к двум разным местам:
//! на первый отвечает таблица маршрутов `State::previews`, на второй —
//! `Latest` внутри самого вида. Вкладку могли закрыть, пока ответ шёл,
//! и тогда ответ наш, а показывать его негде.
//!
//! Формат не проверяем: его определяет по содержимому image-loader, и второй
//! список расширений здесь был бы вторым источником правды (и разошёлся бы
//! с первым). Нераспознанный файл вернётся ошибкой и покажется на экране.

use crate::module::state::{State, ViewId};
use crate::proto::image_loader::{LoadImageRequest, LoadImageResult};
use veldsdk::Reply;
use veldsdk::proto::core::ResourceOpened;

/// Просмотр скачанного файла: открывает библиотека — файл её, и где он лежит,
/// знает только она.
pub fn on_view_local_pressed(state: &mut State, name: String) {
    if name.is_empty() { return; }

    // Один correlation_id на оба шага: по нему же отменяется декодирование.
    let correlation_id = begin_open(state, name.clone(), Some(name.clone()));
    crate::calls::data_library::on_open(&crate::proto::data_library::OpenRequest {
        name,
    }, &correlation_id);
}

/// Просмотр ещё не скачанного файла. Ресурс открывает data-provider (подписать
/// запрос к хранилищу может только он) и передаёт нам владение; дальше путь
/// тот же, что у локального файла: read-грант загрузчику и декод по фрагментам
/// — по проводу идёт только то, что декодер действительно прочитал.
pub fn on_view_remote_pressed(state: &mut State, identifier: String) {
    if identifier.is_empty() { return; }

    // Записи библиотеки за таким снимком нет: он ещё в хранилище.
    let correlation_id = begin_open(state, identifier.clone(), None);
    crate::calls::data_provider::on_open(&crate::proto::data_provider::OpenRequest {
        identifier,
    }, &correlation_id);
}

/// Общее начало обоих путей: новая вкладка и корреляция, по которой её найдёт
/// ответ.
fn begin_open(state: &mut State, label: String, entry: Option<String>) -> String {
    let view = super::nav::open_preview(state, label, entry);
    let correlation_id = state.preview_mut(view)
        .expect("вид только что открыт")
        .begin();
    state.previews.insert(correlation_id.clone(), view);
    correlation_id
}

/// Неудача превью: на экран и в лог. Экран видит только тот, кто в этот момент
/// на него смотрит, — а причина отказа (истёкшая подпись, неподдерживаемый
/// формат) нужна и после того, как пользователь ушёл на другую вкладку.
fn fail(state: &mut State, view: ViewId, error: String) {
    let Some(preview) = state.preview_mut(view) else { return };
    veldsdk::log::warn!(target: "handlers", "превью '{}': {}", preview.label, error);
    preview.error = Some(error);
}

/// Ресурс открыт — неважно кем: библиотекой (скачанный файл) или провайдером
/// (ещё не скачанный, читается по сети). Дальше разницы нет.
///
/// Устаревший ответ (вкладку закрыли или запрос вытеснили) всё равно наш:
/// ресурс уже принадлежит нам, и бросить его значит потерять и регион, и
/// открытый на той стороне дескриптор. `false` — ответ не наш.
pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation_id = veldsdk::correlation();
    // Смотрим, не снимая с учёта: у актуального запроса впереди второй ответ —
    // от загрузчика, и он приедет с той же корреляцией.
    let Some(&mut view) = state.previews.peek(&correlation_id) else { return false };

    let current = match state.preview_mut(view) {
        Some(preview) => preview.request.status(&correlation_id) == Reply::Current,
        // Вкладку закрыли, пока ответ шёл.
        None => false,
    };

    if !current {
        // Показывать нечего, но ресурс уже наш, и второго ответа по нему не
        // будет: операция кончается здесь.
        finish(state, view, &correlation_id);
        if let Some(handle) = &opened.handle {
            veldsdk::resource::release(handle.clone());
        }
        return true;
    }

    match veldsdk::resource::accept(opened) {
        Ok(handle) => start_decode(state, view, handle, correlation_id),
        Err(error) => {
            // Операция кончилась здесь: задачи на этой фазе ещё нет,
            // закрывать нечего.
            finish(state, view, &correlation_id);
            fail(state, view, error);
        }
    }
    true
}

/// Отдаём открытый ресурс загрузчику. Владение остаётся у нас, поэтому и
/// закрываем его мы — после ответа, каким бы он ни был.
fn start_decode(state: &mut State, view: ViewId, resource: veldsdk::ResourceHandle, correlation_id: String) {
    // Грант до постановки ресурса на хранение: при отказе хелпер уже
    // освободил его сам, и второго ответа не будет.
    if let Err(error) = veldsdk::resource::grant_read_or_free(resource.id, "image-loader") {
        finish(state, view, &correlation_id);
        fail(state, view, error);
        return;
    }

    // Бокс превью — размер окна в физических пикселях: больше на экран всё
    // равно не поместится, а декодировать в полный размер снимка незачем.
    let (max_width, max_height) = state.window;
    let Some(preview) = state.preview_mut(view) else { return };
    preview.file = Some(veldsdk::OwnedResource::new(resource.clone()));
    // Загрузчик получил безымянный ресурс — назвать источник в логах и в
    // списке задач можем только мы.
    let label = preview.label.clone();

    crate::calls::image_loader::on_load(&LoadImageRequest {
        resource: Some(resource),
        max_width,
        max_height,
        label,
    }, &correlation_id);
}

/// Ответ image-loader — терминальный, снимаем запрос с учёта. Устаревший
/// (пока ответ шёл, вкладку закрыли) — всё равно наш: текстуру освобождаем на
/// месте, владение уже передано нам, и потерять её значит потерять
/// видеопамять. Чужую не трогаем.
pub fn on_load_result(state: &mut State, result: LoadImageResult) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.previews.take(&correlation_id) else { return };

    let current = match state.preview_mut(view) {
        Some(preview) => preview.request.settle(&correlation_id) == Reply::Current,
        None => false,
    };
    if !current {
        // Показывать уже нечего, но текстура наша — владение передано нам.
        if let Some(handle) = result.handle {
            veldsdk::resource::release(handle);
        }
        return;
    }

    // Файл больше не нужен: декодирование кончилось (успехом или нет).
    // Учёт операции снял хост, приняв этот ответ: он терминальный по схеме
    // загрузчика. Закрывать здесь нечего.
    let Some(preview) = state.preview_mut(view) else { return };
    preview.close_file();
    // Размеры — единственное, что о снимке известно помимо самой картинки:
    // исходник мы не читали, а текстура уже уменьшена загрузчиком.
    preview.source_size = (result.source_width > 0).then_some((result.source_width, result.source_height));
    preview.preview_size = (result.width > 0).then_some((result.width, result.height));

    // ui-service строит view/bind group этой текстуры по read-гранту —
    // тот же ритуал, что grant_write оконной поверхности (surface.rs).
    let accepted = veldsdk::resource::accept_parts(result.handle, &result.error)
        .and_then(|handle| {
            veldsdk::resource::grant_read_or_free(handle.id, "ui-service").map(|()| handle)
        });

    match accepted {
        Ok(handle) => {
            if let Some(preview) = state.preview_mut(view) {
                preview.texture = Some(veldsdk::OwnedResource::new(handle));
            }
        }
        Err(error) => fail(state, view, error),
    }
}

/// Масштаб показа активного снимка. Ноль — вписать в окно; остальное —
/// доля от натурального размера превью.
pub fn on_zoom(state: &mut State, zoom: f32) {
    let Some(id) = state.active_id() else { return };
    if let Some(preview) = state.preview_mut(id) {
        preview.zoom = zoom.clamp(0.0, 8.0);
    }
}

/// Конец операции: убрать маршрут и снять запрос с учёта у вида, если он ещё
/// открыт. Оба факта об одном и том же, и разъехаться им нельзя.
fn finish(state: &mut State, view: ViewId, correlation_id: &str) {
    state.previews.take(correlation_id);
    if let Some(preview) = state.preview_mut(view) {
        preview.request.settle(correlation_id);
    }
}
