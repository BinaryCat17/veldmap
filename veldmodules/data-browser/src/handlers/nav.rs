use crate::module::state::{State, Screen};
use crate::proto::data_library::LibraryRequest;
use crate::proto::ui_service::proto::UiEventResponse;

pub fn on_nav_browse(state: &mut State, _event: UiEventResponse) {
    go_to(state, Screen::Browse);
    super::browse::request_path(state, state.browse.current_path.clone());
}

pub fn on_nav_search(state: &mut State, _event: UiEventResponse) {
    go_to(state, Screen::Search);
}

pub fn on_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    go_to(state, Screen::Downloaded);
}

/// Единственный переход между экранами: уход с превью освобождает его
/// текстуру. Отдельный «выход» на каждой кнопке рано или поздно разъедется —
/// достаточно забыть его на одной.
///
/// Состояние библиотеки заодно перечитывается на каждом переходе: оно нужно
/// всем трём экранам (на Browse/Search — чтобы показать, что уже скачано),
/// и это единственное место, где такой запрос уместен. В остальное время
/// библиотека рассылает изменения сама.
fn go_to(state: &mut State, screen: Screen) {
    if state.current_screen == Screen::Preview && screen != Screen::Preview {
        super::preview::abandon(state);
    }
    state.current_screen = screen;
    request_library();
}

/// Попросить библиотеку перечитать каталог. Ответ придёт обычной рассылкой
/// on_state — своей версии правды о скачанном мы не держим и принимаем
/// любое присланное состояние.
pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
