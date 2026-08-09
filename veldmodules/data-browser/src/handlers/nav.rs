//! Вкладки: открыть вид, показать открытый, закрыть.

use crate::module::state::{BrowseState, PreviewState, SearchState, State, ViewId, ViewKind};
use crate::proto::data_library::LibraryRequest;
use crate::proto::ui_service::proto::UiEventResponse;

/// Кнопки шапки показывают уже открытый вид такого рода, а не заводят второй:
/// Search и Downloaded смысла размножать не имеют — их содержимое от вкладки
/// не зависит. Browse размножать смысл есть (две папки рядом), но заводит
/// вторую вкладку тот, кто этого явно просит, а не общая кнопка «Browse».
pub fn on_nav_browse(state: &mut State, _event: UiEventResponse) {
    if let Some(id) = state.find(|kind| matches!(kind, ViewKind::Browse(_))) {
        state.focus(id);
        return;
    }
    let id = state.open(ViewKind::Browse(BrowseState::default()));
    super::browse::request_path(state, id, String::new());
}

pub fn on_nav_search(state: &mut State, _event: UiEventResponse) {
    match state.find(|kind| matches!(kind, ViewKind::Search(_))) {
        Some(id) => state.focus(id),
        None => {
            state.open(ViewKind::Search(SearchState::default()));
        }
    }
}

pub fn on_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    match state.find(|kind| matches!(kind, ViewKind::Downloaded)) {
        Some(id) => state.focus(id),
        None => {
            state.open(ViewKind::Downloaded);
            // Перечитываем каталог: это единственный момент, когда его просят
            // показать явно. В остальное время библиотека рассылает изменения
            // сама, и своей версии правды о скачанном мы не держим.
            request_library();
        }
    }
}

pub fn on_tab_select(state: &mut State, event: UiEventResponse) {
    if let Some(id) = parse_view_id(&event.value) {
        state.focus(id);
    }
}

/// Закрытие вкладки — единственный выход из вида, поэтому уборка за ним тоже
/// одна: превью гасит декодирование, если оно ещё идёт. Ресурсы вида (файл и
/// текстура) освобождаются вместе с ним — их держит `OwnedResource`.
///
/// Учёт запроса при этом не снимается: ответ по нему придёт всё равно и придёт
/// нам во владение, а опознать его как свой можно только по таблице маршрутов
/// (см. State::previews).
pub fn on_tab_close(state: &mut State, event: UiEventResponse) {
    let Some(id) = parse_view_id(&event.value) else { return };
    let Some(view) = state.close(id) else { return };

    if let ViewKind::Preview(mut preview) = view.kind {
        if let Some(correlation_id) = preview.reset() {
            crate::cancel::image_loader::on_load(&correlation_id);
        }
    }
}

fn parse_view_id(value: &str) -> Option<ViewId> {
    match value.parse() {
        Ok(id) => Some(id),
        Err(_) => {
            veldsdk::log::warn!(target: "handlers", "вкладка названа непонятным id: '{}'", value);
            None
        }
    }
}

/// Открывает превью новой вкладкой: смотреть два снимка по очереди, не теряя
/// первый, — обычное дело, а вкладка ровно для этого и есть.
pub fn open_preview(state: &mut State, label: String) -> ViewId {
    let mut preview = PreviewState::default();
    preview.current_path = label;
    state.open(ViewKind::Preview(preview))
}

/// Попросить библиотеку перечитать каталог. Ответ придёт обычной рассылкой
/// on_state — своей версии правды о скачанном мы не держим и принимаем
/// любое присланное состояние.
pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
