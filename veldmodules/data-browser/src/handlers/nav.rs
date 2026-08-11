//! Вкладки: открыть вид, показать открытый, закрыть.

use crate::module::state::{BrowseState, PreviewState, SearchState, State, ViewId, ViewKind};
use crate::proto::data_library::LibraryRequest;

/// Каталог всегда открывается новой вкладкой: две папки рядом — обычное дело,
/// и ровно для этого вкладки и есть.
pub fn on_new_browse(state: &mut State) {
    state.tab_menu = false;
    let id = state.open(ViewKind::Browse(BrowseState::default()));
    super::browse::request_path(state, id, String::new());
}

/// Поиск и скачанное показывают уже открытую вкладку, а не заводят вторую: их
/// содержимое от вкладки не зависит, и второй такой же список — просто копия.
pub fn on_new_search(state: &mut State) {
    state.tab_menu = false;
    match state.find(|kind| matches!(kind, ViewKind::Search(_))) {
        Some(id) => state.focus(id),
        None => {
            state.open(ViewKind::Search(SearchState::default()));
            // Пустой запрос — это «самое свежее», ровно как обещает подсказка
            // поля. Без него открытая вкладка пишет «ничего не нашлось» про то,
            // чего никто не искал.
            super::search::run(state);
        }
    }
}

pub fn on_new_downloaded(state: &mut State) {
    state.tab_menu = false;
    match state.find(|kind| matches!(kind, ViewKind::Downloaded(_))) {
        Some(id) => state.focus(id),
        None => {
            state.open(ViewKind::Downloaded(Default::default()));
            // Перечитываем каталог: это единственный момент, когда его просят
            // показать явно. В остальное время библиотека рассылает изменения
            // сама, и своей версии правды о скачанном мы не держим.
            request_library();
        }
    }
}

/// Глобус, как поиск и скачанное, показывает уже открытую вкладку: шар один,
/// и второй такой же — просто вторая камера над тем же местом. К тому же
/// каждая вкладка держит своё место под рендер, а рисующий модуль один.
pub fn on_new_globe(state: &mut State) {
    state.tab_menu = false;
    match state.find(|kind| matches!(kind, ViewKind::Globe(_))) {
        Some(id) => state.focus(id),
        None => {
            state.open(ViewKind::Globe(Default::default()));
        }
    }
}

/// Меню полосы вкладок. Раскрытое меню списка при этом закрывается — открытым
/// бывает только одно.
pub fn on_tab_menu(state: &mut State, open: bool) {
    state.tab_menu = open;
    if let Some(listing) = state.active_listing_mut() {
        listing.menu = crate::module::state::listing::Menu::Closed;
    }
}

pub fn on_tab_select(state: &mut State, id: ViewId) {
    state.tab_menu = false;
    state.focus(id);
}

/// Закрытие вкладки — единственный выход из вида, поэтому здесь и уборка за
/// ним. Ресурсы вида (файл, текстура) освобождаются вместе с ним сами — их
/// держит `OwnedResource`; убирать приходится ровно то, о чём знает кто-то
/// ещё: незаконченная работа и отданное чужому модулю место.
///
/// Учёт запроса при этом не снимается: ответ по нему придёт всё равно и придёт
/// нам во владение, а опознать его как свой можно только по таблице маршрутов
/// (см. State::previews).
pub fn on_tab_close(state: &mut State, id: ViewId) {
    let Some(view) = state.close(id) else { return };

    match view.kind {
        ViewKind::Preview(mut preview) => {
            if let Some(correlation_id) = preview.reset() {
                crate::cancel::image_loader::on_load(&correlation_id);
            }
        }
        // Место под шар освободится своим Drop, но глобусу об этом надо
        // сказать: у него остался view этой текстуры, и молча освобождённую он
        // продолжил бы рисовать до конца процесса.
        ViewKind::Globe(globe) => {
            veldsdk::surface::revoke(globe.surface, crate::calls::globe::on_set_surface);
        }
        _ => {}
    }
}

/// Открывает превью новой вкладкой: смотреть два снимка по очереди, не теряя
/// первый, — обычное дело, а вкладка ровно для этого и есть.
pub fn open_preview(state: &mut State, label: String, entry: Option<String>) -> ViewId {
    state.open(ViewKind::Preview(PreviewState { label, entry, ..Default::default() }))
}

/// Запрашивает всё, чем живёт стартовая вкладка. Отдельно от `State::new`:
/// там модуль ещё не на шине, и публиковать некуда.
pub fn bootstrap(state: &mut State) {
    request_library();

    // Стартовая вкладка каталога пуста, пока её не спросили: содержимое
    // приходит ответом, а не лежит в состоянии.
    let empty: Vec<ViewId> = state
        .views()
        .iter()
        .filter(|view| matches!(view.kind, ViewKind::Browse(_)))
        .map(|view| view.id)
        .collect();
    for id in empty {
        super::browse::request_path(state, id, String::new());
    }
}

/// Попросить библиотеку перечитать каталог. Ответ придёт обычной рассылкой
/// on_state — своей версии правды о скачанном мы не держим и принимаем
/// любое присланное состояние.
pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
