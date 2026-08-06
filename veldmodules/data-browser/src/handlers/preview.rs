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
use veldsdk::Reply;
use veldsdk::proto::core::ResourceOpened;

/// Бросает текущее превью: освобождает ресурсы и убивает декодирование, если
/// оно ещё идёт. Снимок декодируется секундами, и продолжать работу ради
/// картинки, которую уже никто не увидит, незачем.
///
/// Не разбираем, дошло ли дело до декодирования: копия учёта, который и так
/// ведёт платформа, разошлась бы с ним, а убийство того, чего уже нет, —
/// нормальный исход.
pub fn abandon(state: &mut State) {
    if let Some(task_id) = state.preview.reset() {
        crate::cancel::image_loader::on_load(&task_id);
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
    }, &correlation_id);
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
    }, &correlation_id);
}

/// Неудача превью: на экран и в лог. Экран видит только тот, кто в этот момент
/// на него смотрит, — а причина отказа (истёкшая подпись, неподдерживаемый
/// формат) нужна и после того, как пользователь ушёл на другой экран.
fn fail(state: &mut State, error: String) {
    veldsdk::log::warn!(target: "handlers", "превью '{}': {}", state.preview.current_path, error);
    state.preview.error = Some(error);
}

/// Общее начало обоих путей: экран превью, отказ от предыдущего, новый id.
fn begin_open(state: &mut State, label: String) -> String {
    state.current_screen = Screen::Preview;
    abandon(state);
    state.preview.current_path = label;

    state.preview.begin()
}

/// Ресурс открыт — неважно кем: библиотекой (скачанный файл) или провайдером
/// (ещё не скачанный, читается по сети). Дальше разницы нет.
///
/// Устаревший ответ (пользователь успел уйти с экрана или открыть другое)
/// всё равно наш: ресурс уже принадлежит нам, и бросить его значит потерять
/// и регион, и открытый на той стороне дескриптор. `false` — ответ не наш.
pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation_id = veldsdk::correlation();
    // Не снимаем с учёта: у актуального запроса впереди второй ответ — от
    // загрузчика, и он опознаётся той же корреляцией.
    match state.preview.request.status(&correlation_id) {
        Reply::Foreign => return false,
        Reply::Stale => {
            // Запрос вытеснен, показывать его нечего — но ресурс уже наш,
            // и второго ответа по нему не будет: операция кончается здесь.
            state.preview.request.settle(&correlation_id);
            if let Some(handle) = &opened.handle {
                drop(veldsdk::OwnedResource::new(handle.clone()));
            }
            return true;
        }
        Reply::Current => {}
    }

    match veldsdk::resource::accept(opened) {
        Ok(handle) => start_decode(state, handle, correlation_id),
        Err(error) => {
            // Операция кончилась здесь: задачи на этой фазе ещё нет,
            // закрывать нечего.
            state.preview.request.settle(&correlation_id);
            fail(state, error);
        }
    }
    true
}

/// Отдаём открытый ресурс загрузчику. Владение остаётся у нас, поэтому и
/// закрываем его мы — после ответа, каким бы он ни был.
fn start_decode(state: &mut State, resource: veldsdk::ResourceHandle, correlation_id: String) {
    // Грант до постановки ресурса на хранение: при отказе хелпер уже
    // освободил его сам, и второго ответа не будет.
    if let Err(error) = veldsdk::resource::grant_read_or_free(resource.id, "image-loader") {
        state.preview.request.settle(&correlation_id);
        fail(state, error);
        return;
    }
    state.preview.file = Some(veldsdk::OwnedResource::new(resource.clone()));

    // Бокс превью — размер окна в физических пикселях: больше на экран всё
    // равно не поместится, а декодировать в полный размер снимка незачем.
    crate::calls::image_loader::on_load(&LoadImageRequest {
        resource: Some(resource),
        max_width: state.window.0,
        max_height: state.window.1,
        // Загрузчик получил безымянный ресурс — назвать источник в логах и в
        // списке задач можем только мы.
        label: state.preview.current_path.clone(),
    }, &correlation_id);
}

/// Ответ image-loader — терминальный, снимаем запрос с учёта. Устаревший
/// (пока ответ шёл, пользователь ушёл с экрана или открыл другой файл) — всё
/// равно наш: текстуру освобождаем на месте, владение уже передано нам, и
/// потерять её значит потерять видеопамять. Чужую не трогаем.
pub fn on_load_result(state: &mut State, result: LoadImageResult) {
    let correlation_id = veldsdk::correlation();
    match state.preview.request.settle(&correlation_id) {
        Reply::Current => {}
        Reply::Stale => {
            // Показывать уже нечего, но текстура наша — владение передано нам.
            if let Some(handle) = result.handle {
                drop(veldsdk::OwnedResource::new(handle));
            }
            return;
        }
        // Чужой ответ: текстура не наша, трогать её нельзя.
        Reply::Foreign => return,
    }
    // Файл больше не нужен: декодирование кончилось (успехом или нет).
    state.preview.close_file();
    // Учёт операции снял хост, приняв этот ответ: он терминальный по схеме
    // загрузчика. Закрывать здесь нечего.

    let handle = match veldsdk::resource::accept_parts(result.handle, &result.error) {
        Ok(handle) => handle,
        Err(error) => {
            fail(state, error);
            return;
        }
    };

    // ui-service строит view/bind group этой текстуры по read-гранту —
    // тот же ритуал, что grant_write оконной поверхности (surface.rs).
    if let Err(error) = veldsdk::resource::grant_read_or_free(handle.id, "ui-service") {
        fail(state, error);
        return;
    }
    state.preview.texture = Some(veldsdk::OwnedResource::new(handle));
}
