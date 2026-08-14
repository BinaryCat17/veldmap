//! Вкладки: открыть вид, показать открытый, закрыть.

use crate::module::state::{
    BrowseState, Half, Placement, PreviewState, SearchState, State, ViewId, ViewKind,
};
use crate::module::NewTab;
use crate::proto::data_library::LibraryRequest;

/// Завести вкладку в названной половине.
///
/// Каталог всегда открывается новой: две папки рядом — обычное дело, и ровно
/// для этого вкладки и есть. Остальные — синглтоны, и уже открытый показывается
/// вместо второго такого же; поиск и скачанное потому, что их содержимое от
/// вкладки не зависит, глобус — потому, что рисующий модуль один, а «На
/// просмотре» — потому, что слои лежат в состоянии модуля, а не вида.
///
/// Синглтон, найденный в другой половине, переезжает в ту, из которой его
/// позвали: просили показать его здесь, а не отправить взгляд туда.
pub fn on_new_tab(state: &mut State, half: Half, kind: NewTab) {
    state.close_menus();
    if let NewTab::Browse = kind {
        let id = state.open_in(half, ViewKind::Browse(BrowseState::default()));
        super::browse::request_path(state, id, String::new());
        return;
    }

    let matches = |view: &ViewKind| match kind {
        NewTab::Browse => false,
        NewTab::Search => matches!(view, ViewKind::Search(_)),
        NewTab::Downloaded => matches!(view, ViewKind::Downloaded(_)),
        NewTab::Globe => matches!(view, ViewKind::Globe(_)),
        NewTab::Shown => matches!(view, ViewKind::Shown),
    };
    if let Some(id) = state.find(matches) {
        state.move_to(id, half);
        state.focus(id);
        return;
    }

    match kind {
        NewTab::Browse => unreachable!("каталог открыт выше"),
        NewTab::Search => {
            let id = state.open_in(half, ViewKind::Search(SearchState::default()));
            // Пустой запрос — это «самое свежее», ровно как обещает подсказка
            // поля. Без него открытая вкладка пишет «ничего не нашлось» про то,
            // чего никто не искал.
            super::search::run(state, id);
        }
        NewTab::Downloaded => {
            state.open_in(half, ViewKind::Downloaded(Default::default()));
            // Перечитываем каталог: это единственный момент, когда его просят
            // показать явно. В остальное время библиотека рассылает изменения
            // сама, и своей версии правды о скачанном мы не держим.
            request_library();
        }
        NewTab::Globe => {
            state.open_in(half, ViewKind::Globe(Default::default()));
        }
        NewTab::Shown => {
            state.open_in(half, ViewKind::Shown);
        }
    }
}

/// Показать вкладку глобуса — её открывает всякий показ снимка на шаре.
///
/// Открытую не трогаем, даже если она в другой половине: снимок кладут, глядя
/// на неё, и утащить её к списку значило бы убрать с экрана то, ради чего всё и
/// затевалось. Нет вовсе — заводим в той половине, что под рукой.
pub fn on_new_globe(state: &mut State) {
    match state.find(|kind| matches!(kind, ViewKind::Globe(_))) {
        Some(id) => state.focus(id),
        None => on_new_tab(state, state.focused(), NewTab::Globe),
    }
}

/// Меню «плюса» одной из половин. Раскрытое меню списка при этом закрывается —
/// открытым бывает только одно (см. State::close_menus).
pub fn on_tab_menu(state: &mut State, half: Option<Half>) {
    state.close_menus();
    state.tab_menu = half;
}

/// Меню самой вкладки: разделить, перенести, закрыть.
pub fn on_tab_options(state: &mut State, id: Option<ViewId>) {
    state.close_menus();
    state.tab_options = id;
}

/// Разделить экран, отправив вкладку во вторую половину.
pub fn on_tab_split(state: &mut State, id: ViewId, placement: Placement) {
    state.close_menus();
    state.split_off(id, placement);
}

/// Перенести вкладку в другую половину. Экран при этом делится, если ещё не
/// разделён: переносить некуда, пока половина одна.
pub fn on_tab_move(state: &mut State, id: ViewId) {
    state.close_menus();
    let Some(view) = state.views().iter().find(|view| view.id == id) else { return };
    let target = view.half.other();
    // Само разделение вкладок не двигает, поэтому на неразделённом экране это
    // два шага: сперва завести вторую половину, потом перенести в неё.
    if state.split().is_none() {
        state.split_off(id, Placement::Right);
    }
    state.move_to(id, target);
}

/// Свести половины обратно в одну.
pub fn on_tab_unsplit(state: &mut State) {
    state.close_menus();
    state.unsplit();
}

pub fn on_tab_select(state: &mut State, id: ViewId) {
    state.close_menus();
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
        // Показ ведёт канва — ей и сказать, что вида больше нет: она убьёт
        // своё производство и отпустит ресурс. Место освободится нашим Drop —
        // после этого события, чтобы канва не рисовала в отозванное.
        ViewKind::Preview(_) => {
            crate::calls::image_view::on_close(&crate::proto::image_view::CloseView {
                view: id.to_string(),
            });
        }
        // Место под шар освободится своим Drop, но глобусу об этом надо
        // сказать: у него остался view этой текстуры, и молча освобождённую он
        // продолжил бы рисовать до конца процесса.
        ViewKind::Globe(globe) => {
            veldsdk::surface::revoke(globe.surface, crate::calls::globe::on_set_surface);
        }
        // Контуры этого поиска могли уйти на шар — выделение там гаснет,
        // выбор по щелчку отключается (см. search::on_source_closed).
        ViewKind::Search(search) => {
            super::search::on_source_closed(state, id, &search);
        }
        _ => {}
    }

    // Пустая половина сама по себе законна — она предлагает, что в неё
    // положить, — но пустыми обе быть не могут: делить нечего.
    if state.views().is_empty() {
        state.unsplit();
    }
}

/// Открывает превью новой вкладкой — в той половине, где стоит строка, из
/// которой его позвали: смотреть рядом со списком и есть то, ради чего экран
/// делят. Смотреть два снимка по очереди, не теряя первый, — обычное дело, а
/// вкладка ровно для этого и есть.
pub fn open_preview(
    state: &mut State,
    half: Half,
    label: String,
    entry: Option<String>,
) -> ViewId {
    state.open_in(half, ViewKind::Preview(PreviewState { label, entry, ..Default::default() }))
}

/// Половина, в которой лежит вкладка; для закрытой — та, что под рукой.
pub fn half_of(state: &State, id: ViewId) -> Half {
    state.views().iter().find(|view| view.id == id).map_or(state.focused(), |view| view.half)
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
