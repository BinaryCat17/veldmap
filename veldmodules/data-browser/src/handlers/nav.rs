//! Вкладки: открыть вид, показать открытый, перенести, закрыть.

use crate::module::state::{
    BrowseState, PaneId, PreviewState, SearchState, Side, State, ViewId, ViewKind,
};
use crate::module::NewTab;
use crate::proto::data_library::LibraryRequest;
use crate::proto::ui_service::{DropEdge, DropEvent};

/// Завести вкладку в названной панели.
///
/// Синглтон, найденный в другой панели, переезжает в ту, из которой его
/// позвали: просили показать его здесь, а не отправить взгляд туда.
pub fn on_new_tab(state: &mut State, pane: PaneId, kind: NewTab) {
    state.close_menus();
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        return;
    }
    // Всякая вкладка заводится пустой, а вид ей даёт `start` — тем же ходом,
    // которым наполняют оставленную пустой (см. [`fill`]).
    let id = state.open_in(pane, ViewKind::Empty);
    start(state, id, kind);
}

/// Наполнить пустую вкладку выбранным видом: она и есть тот вопрос, ответом на
/// который это приходит.
///
/// Наполняется она на месте, а не заводит рядом вторую: пустая вкладка — это
/// уже занятое место в полосе, и оставлять её за собой значило бы копить
/// вопросы, на которые уже ответили.
pub fn fill(state: &mut State, id: ViewId, kind: NewTab) {
    state.close_menus();
    let Some(pane) = state.pane_of(id) else { return };
    // Синглтон, открытый где-то ещё, переезжает сюда, а пустая вкладка уходит:
    // она была просьбой «покажи это здесь», и двух ответов у неё нет. Сперва
    // переезд, потом закрытие — панель не должна опустеть между ними, иначе
    // она уйдёт из дерева вместе с местом, куда переезжают.
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        on_tab_close(state, id);
        return;
    }
    start(state, id, kind);
}

/// Уже открытый вид, второму такому же не бывать. `None` — такой род заводят
/// сколько угодно раз.
///
/// Каталог заводят сколько угодно: две папки рядом — обычное дело, и ровно для
/// этого вкладки и есть; пустая вкладка — тем более, это место, а не вид.
/// Остальные — синглтоны: поиск и скачанное потому, что их содержимое от
/// вкладки не зависит, глобус — потому, что рисующий модуль один, а «На
/// просмотре» — потому, что слои лежат в состоянии модуля, а не вида.
pub(super) fn singleton(state: &State, kind: NewTab) -> Option<ViewId> {
    state.find(|view| match kind {
        NewTab::Browse | NewTab::Empty => false,
        NewTab::Search => matches!(view, ViewKind::Search(_)),
        NewTab::Downloaded => matches!(view, ViewKind::Downloaded(_)),
        NewTab::Globe => matches!(view, ViewKind::Globe(_)),
        NewTab::Shown => matches!(view, ViewKind::Shown),
    })
}

/// Даёт вкладке вид и просит то, чем этот вид живёт.
fn start(state: &mut State, id: ViewId, kind: NewTab) {
    let fresh = match kind {
        NewTab::Empty => ViewKind::Empty,
        NewTab::Browse => ViewKind::Browse(BrowseState::default()),
        NewTab::Search => ViewKind::Search(SearchState::default()),
        NewTab::Downloaded => ViewKind::Downloaded(Default::default()),
        NewTab::Globe => ViewKind::Globe(Default::default()),
        NewTab::Shown => ViewKind::Shown,
    };
    let Some(place) = state.get_mut(id) else { return };
    *place = fresh;

    // Содержимое приходит ответом, а не лежит в состоянии, — поэтому вслед за
    // видом уходит и первый запрос.
    match kind {
        NewTab::Browse => super::browse::request_path(state, id, String::new()),
        // Пустой запрос — это «самое свежее», ровно как обещает подсказка поля.
        // Без него открытая вкладка пишет «ничего не нашлось» про то, чего
        // никто не искал.
        NewTab::Search => super::search::run(state, id),
        // Перечитываем каталог: это единственный момент, когда его просят
        // показать явно. В остальное время библиотека рассылает изменения
        // сама, и своей версии правды о скачанном мы не держим.
        NewTab::Downloaded => request_library(),
        NewTab::Empty | NewTab::Globe | NewTab::Shown => {}
    }
}

/// Показать вкладку глобуса — её открывает всякий показ снимка на шаре.
///
/// Открытую не трогаем, даже если она в другой панели: снимок кладут, глядя на
/// неё, и утащить её к списку значило бы убрать с экрана то, ради чего всё и
/// затевалось. Нет вовсе — заводим в той панели, что под рукой.
pub fn on_new_globe(state: &mut State) {
    match state.find(|kind| matches!(kind, ViewKind::Globe(_))) {
        Some(id) => state.focus(id),
        None => on_new_tab(state, state.focused(), NewTab::Globe),
    }
}

/// Каталог, в который вести переход, — и открытие нового, если каталога нет
/// вовсе.
///
/// Открытый переиспользуется, потому что переход — это не «заведи мне
/// каталог», а «покажи мне это место»: заводить под каждый переход по вкладке
/// значит копить их, пока за ними не станет не видно работы. Новый каталог
/// по-прежнему открывает тот, кто его и просил, — меню «плюса» (см.
/// [`on_new_tab`]).
///
/// Порядок предпочтения — от ближнего к дальнему: показанный в этой панели,
/// любой в ней, любой в остальных. Ближний потому, что смотрят сюда; открытая
/// в этой панели вкладка ещё и не двигает взгляд. Переносить найденный в чужой
/// панели не станем — это тот же случай, что у глобуса: взгляд переводят к
/// нему, а не тащат его к списку.
pub fn catalog(state: &mut State, from: ViewId) -> ViewId {
    let here = pane_of(state, from);
    let is_browse = |kind: &ViewKind| matches!(kind, ViewKind::Browse(_));

    let shown = state.active_in(here).filter(|id| state.get(*id).is_some_and(is_browse));
    let found = shown
        .or_else(|| {
            state.views_in(here).find(|view| is_browse(&view.kind)).map(|view| view.id)
        })
        .or_else(|| state.find(is_browse));

    match found {
        Some(id) => {
            state.focus(id);
            id
        }
        None => state.open_in(here, ViewKind::Browse(BrowseState::default())),
    }
}

/// Показать запись в каталоге: открыть папку, в которой она лежит, встать на
/// её страницу и подсветить её.
///
/// Один обработчик на все переходы к записи — из строки списка, из полосы под
/// шаром, из «На просмотре»: вопрос у них один, и три ответа на него разошлись
/// бы тем, куда каждый кладёт вкладку.
pub fn in_catalog(state: &mut State, from: ViewId, key: String) {
    state.close_menus();
    if key.is_empty() {
        return;
    }
    let view = catalog(state, from);
    super::browse::reveal(state, view, key);
}

/// Меню «плюса» одной из панелей. Раскрытое меню списка при этом закрывается —
/// открытым бывает только одно (см. State::close_menus).
pub fn on_tab_menu(state: &mut State, pane: Option<PaneId>) {
    state.close_menus();
    state.tab_menu = pane;
}

/// Меню самой вкладки: перенести, закрыть.
pub fn on_tab_options(state: &mut State, id: Option<ViewId>) {
    state.close_menus();
    state.tab_options = id;
}

/// Перенести вкладку туда, где человек видит эту сторону.
pub fn on_tab_move(state: &mut State, id: ViewId, side: Side) {
    state.close_menus();
    state.move_aside(id, side);
}

/// Вкладку принесли мышью и бросили: в середину панели — значит положить в
/// неё, в край — значит поделить ею место с этой стороны.
///
/// Нагрузку разбирает обработчик, а не кодек: изготавливает её рендерер (см.
/// `Msg::TabDrop`), и разбирать её значит переводить его слова — край зоны — в
/// наши, в сторону деления.
pub fn on_tab_drop(state: &mut State, pane: PaneId, drop: DropEvent) {
    state.close_menus();
    let Ok(id) = drop.payload.parse::<ViewId>() else { return };
    match side_of(drop.edge()) {
        None => state.move_to(id, pane),
        Some(side) => state.drop_beside(id, pane, side),
    }
}

/// Край зоны — в сторону деления. Середина стороны не называет: в неё кладут,
/// а не делят ею.
fn side_of(edge: DropEdge) -> Option<Side> {
    match edge {
        DropEdge::DropCenter => None,
        DropEdge::DropLeft => Some(Side::Left),
        DropEdge::DropRight => Some(Side::Right),
        DropEdge::DropAbove => Some(Side::Above),
        DropEdge::DropBelow => Some(Side::Below),
    }
}

/// Свести все панели в одну.
pub fn on_tab_collapse(state: &mut State) {
    state.close_menus();
    state.collapse();
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
///
/// Опустевшая панель уходит из дерева сама (см. `State::close`) — закрытие
/// вкладки и есть единственный способ её убрать.
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
        // Отмеченное в этой выдаче было очерчено на шаре, а показанное из
        // неё — наложено: и то, и другое уходит вместе с ней
        // (см. search::on_source_closed).
        ViewKind::Search(_) => {
            super::search::on_source_closed(state, id);
        }
        // Отметки прочих списков живут в них же — закрытая вкладка уносит их
        // с собой, и набор контуров надо пересобрать.
        ViewKind::Browse(_) | ViewKind::Downloaded(_) => {
            super::outline::refresh(state);
        }
        // Слои лежат в состоянии модуля, а не здесь: закрытая вкладка не
        // снимает с шара ничего (см. `ViewKind::Shown`). У пустой убирать и
        // вовсе нечего — своего состояния у неё нет.
        ViewKind::Empty | ViewKind::Shown => {}
    }
}

/// Открывает превью новой вкладкой — в той панели, где стоит строка, из
/// которой его позвали: смотреть рядом со списком и есть то, ради чего экран
/// делят. Смотреть два снимка по очереди, не теряя первый, — обычное дело, а
/// вкладка ровно для этого и есть.
pub fn open_preview(
    state: &mut State,
    pane: PaneId,
    label: String,
    entry: Option<String>,
) -> ViewId {
    state.open_in(pane, ViewKind::Preview(PreviewState { label, entry, ..Default::default() }))
}

/// Панель, в которой лежит вкладка; для закрытой — та, что под рукой.
pub fn pane_of(state: &State, id: ViewId) -> PaneId {
    state.pane_of(id).unwrap_or_else(|| state.focused())
}

/// Первый ход модуля на шине. Отдельно от `State::new`: там он ещё не на ней,
/// и публиковать некуда.
///
/// Вкладок в этот момент нет вовсе — их принесёт сохранённая раскладка, а если
/// вспоминать нечего, откроется названная конфигом (см. handlers::persist).
pub fn bootstrap(state: &mut State) {
    request_library();
    super::persist::load(state);
}

/// Попросить библиотеку перечитать каталог. Ответ придёт обычной рассылкой
/// on_state — своей версии правды о скачанном мы не держим и принимаем
/// любое присланное состояние.
pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
