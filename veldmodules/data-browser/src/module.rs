pub mod footprint;
pub mod handlers;
pub mod message;
pub mod state;
pub mod view;
pub mod components;
pub mod theme;

// -- Types --
pub use handlers::Config;
pub use message::{Msg, NewTab, ViewMsg};
pub use state::State;
use veld_ui_service_wrap::UiMessage;

// -- Init --
pub fn hook_init(config: Config) -> anyhow::Result<State> {
    State::new(config)
}

// -- Event hook (Elm-цикл: вызывается сгенерированным раннером после каждого
// сообщения). Пересобирает view и шлёт в ui-service; неизменившийся layout по
// шине не едет — топик объявлен снимком (см. его schema.yaml). --
pub fn hook_event(state: &State) {
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(root, crate::calls::ui_service::on_set_view);
}

// -- UI-события (ui-service/on_ui_event, адресовано нам через target) --
// Один топик на все виджеты: ui-service возвращает нажатое эхом парой строк и
// смысла их не знает. Разбор — единственное место, где строки снова становятся
// сообщением; дальше по модулю едет уже `Msg`.
pub fn on_ui_event(state: &mut State, event: crate::proto::ui_service::proto::UiEventResponse) {
    let Some(message) = Msg::decode(&event) else {
        veldsdk::log::warn!(target: "handlers", "непонятное сообщение разметки: '{}'", event.method);
        return;
    };

    // Извещение о том, что не вышло, живёт до следующего действия — и гаснет
    // здесь, до разбора: обработчик этого же нажатия вправе поставить своё, и
    // гашение после него стёрло бы свежее (см. `State::notice`).
    //
    // Действия, а не всякого сообщения: указатель над областью и её новый
    // размер приезжают на голое движение мыши, а показ на шаре переводит взгляд
    // как раз на шар — курсор оказывается над той самой областью, и извещение о
    // неудавшемся показе стиралось бы раньше, чем его успевали прочесть.
    if !idle(&message) {
        state.notice = None;
    }

    // Раскладку запоминаем после разбора, но не на том, чего человек не
    // отправлял: граница и область шлют десятки сообщений в секунду, и каждое
    // прогоняло бы снимок всего дерева панелей через serde_json впустую.
    // Отпускание границы сюда не входит — им перетаскивание и кончается, и
    // записать надо ровно то место, где её отпустили (см. [`settled`]).
    let settled = settled(&message);

    match message {
        Msg::TabSelect(id) => handlers::nav::on_tab_select(state, id),
        Msg::TabClose(id) => handlers::nav::on_tab_close(state, id),
        Msg::TabMenu(pane) => handlers::nav::on_tab_menu(state, pane),
        Msg::TabOptions(id) => handlers::nav::on_tab_options(state, id),
        Msg::NewTab(pane, kind) => handlers::nav::on_new_tab(state, pane, kind),
        Msg::TabMove(id, side) => handlers::nav::on_tab_move(state, id, side),
        Msg::TabCollapse => handlers::nav::on_tab_collapse(state),
        Msg::Divide(split, delta) => state.divide(split, delta),
        // Границу отпустили: делать нечего — это тот самый момент, ради
        // которого сообщение и заведено, а запоминает раскладку общий ход ниже.
        Msg::Divided => {}
        Msg::TabDrop(pane, drop) => handlers::nav::on_tab_drop(state, pane, drop),
        Msg::Download(identifier, product) => {
            handlers::library::on_download_pressed(state, identifier, product)
        }
        Msg::Delete(name) => handlers::library::on_delete_pressed(state, name),
        Msg::DeleteSnapshot(product) => handlers::library::on_delete_snapshot(state, product),
        Msg::DownloadSnapshot(product) => handlers::library::on_download_snapshot(state, product),
        Msg::PauseSnapshot(product) => handlers::library::on_pause_snapshot(state, product),
        Msg::Reveal(name) => handlers::library::on_reveal_pressed(state, name),
        // «Снять с шара» — про всё, что на нём лежит: и растры, и контуры.
        // Порознь их не снять ничем, а разделять их пользователю не по чему —
        // он видит один шар.
        Msg::GlobeClear => {
            handlers::overlay::clear_all(state);
            handlers::outline::clear(state);
        }
        Msg::OverlayOpacity(key, value) => handlers::overlay::set_opacity(state, &key, value),
        Msg::OverlayHidden(key, hidden) => handlers::overlay::set_hidden(state, &key, hidden),
        Msg::OverlayRemove(key) => handlers::overlay::remove(state, &key),
        Msg::OverlayShift(key, shift) => handlers::overlay::shift(state, &key, shift),
        Msg::OverlayHideAll(hidden) => handlers::overlay::hide_all(state, hidden),
        Msg::OverlayMenu(key) => handlers::overlay::menu(state, key),
        Msg::OverlayVariables(key) => handlers::overlay::variables_menu(state, key),
        Msg::OverlayVariable(key, variable) => handlers::overlay::variable(state, &key, variable),
        Msg::OutlineToggle(key) => handlers::outline::toggle_outline(state, key),
        Msg::OutlineRemove(key) => handlers::outline::drop_one(state, &key),
        Msg::OutlineFocus(key) => handlers::outline::focus(state, &key),
        Msg::In(view, message) => on_view_message(state, view, message),
    }

    if settled {
        handlers::persist::save_if_changed(state);
    }
}

/// Есть ли раскладке что запоминать после этого сообщения.
///
/// Всё, чего человек не отправлял (см. [`idle`]), раскладки не меняет — кроме
/// отпускания границы: оно и есть конец перетаскивания. Отдельным предикатом,
/// а не своим списком сообщений: два списка одного и того же разошлись бы на
/// первом же новом сообщении, приезжающем «само».
fn settled(message: &Msg) -> bool {
    !idle(message) || matches!(message, Msg::Divided)
}

/// Сообщение, которое человек не отправлял, — оно приезжает от того, что
/// курсор подвинулся или окно поделили. Таким нечего гасить извещения: они
/// говорят о том, что не вышло по нажатию, и стереть их вправе только
/// следующее нажатие.
fn idle(message: &Msg) -> bool {
    matches!(
        message,
        Msg::Divide(..)
            | Msg::Divided
            | Msg::In(_, ViewMsg::GlobePointer(_))
            | Msg::In(_, ViewMsg::GlobeResized(_))
            | Msg::In(_, ViewMsg::PreviewPointer(_))
            | Msg::In(_, ViewMsg::PreviewResized(_))
    )
}

/// Сообщение из тела вкладки. Вид назвал себя сам (см. `Msg::In`), поэтому
/// адресат здесь известен точно — и обработчику не приходится гадать, какая из
/// панелей была под рукой.
fn on_view_message(state: &mut State, view: crate::module::state::ViewId, message: ViewMsg) {
    match message {
        ViewMsg::Fill(kind) => handlers::nav::fill(state, view, kind),
        ViewMsg::OpenMenu(menu) => handlers::listing::on_menu(state, view, menu),
        ViewMsg::Filter(filter) => handlers::listing::on_filter(state, view, filter),
        ViewMsg::Group(grouping) => handlers::listing::on_group(state, view, grouping),
        ViewMsg::Sort(sorting) => handlers::listing::on_sort(state, view, sorting),
        ViewMsg::Query(query) => handlers::listing::on_query(state, view, query),
        ViewMsg::Page(page) => handlers::listing::on_page(state, view, page),
        ViewMsg::Expand(key) => handlers::listing::on_expand(state, view, key),
        ViewMsg::Check(key) => handlers::outline::toggle(state, view, key),
        ViewMsg::CheckShown(on) => handlers::outline::mark_shown(state, view, on),
        ViewMsg::CheckClear => handlers::outline::unmark_all(state, view),
        ViewMsg::CheckDownload => handlers::library::on_download_selected(state, view),
        ViewMsg::CheckDelete => handlers::library::on_delete_selected(state, view),
        ViewMsg::SearchQuery(query) => handlers::search::on_query(state, view, query),
        ViewMsg::SearchMission(mission) => handlers::search::on_mission(state, view, mission),
        ViewMsg::SearchPeriod(period) => handlers::search::on_period(state, view, period),
        ViewMsg::SearchFrom(value) => handlers::search::on_from(state, view, value),
        ViewMsg::SearchTo(value) => handlers::search::on_to(state, view, value),
        ViewMsg::SearchCloud(cloud) => handlers::search::on_cloud(state, view, cloud),
        ViewMsg::RunSearch => handlers::search::run(state, view),
        ViewMsg::Enter(path) => handlers::browse::on_enter(state, view, path),
        ViewMsg::Up => handlers::browse::on_up(state, view),
        ViewMsg::InCatalog(key) => handlers::nav::in_catalog(state, view, key),
        ViewMsg::Preview(name) => handlers::preview::on_view_local_pressed(state, view, name),
        ViewMsg::PreviewProduct(identifier) => {
            handlers::preview::on_view_product_pressed(state, view, identifier)
        }
        ViewMsg::PreviewRemote(identifier) => {
            handlers::preview::on_view_remote_pressed(state, view, identifier)
        }
        ViewMsg::PreviewResized(size) => handlers::preview::on_resized(state, view, size),
        ViewMsg::PreviewPointer(event) => handlers::preview::on_pointer(state, view, event),
        ViewMsg::PreviewFit => handlers::preview::on_fit(state, view),
        ViewMsg::PreviewZoom(direction) => handlers::preview::on_zoom_step(state, view, direction),
        ViewMsg::PreviewVariables(open) => handlers::preview::on_variables(state, view, open),
        ViewMsg::PreviewVariable(variable) => handlers::preview::on_variable(state, view, variable),
        ViewMsg::GlobeResized(size) => handlers::globe::on_resized(state, view, size),
        ViewMsg::GlobePointer(event) => handlers::globe::on_pointer(state, view, event),
        ViewMsg::GlobeToggle(identifier) => handlers::overlay::on_toggle_pressed(state, view, identifier),
    }
}

// -- Sub handlers --
pub use handlers::library::on_state;
pub use handlers::preview::on_view_state;
// Раскладка окна между запусками: файл читается один раз на старте, пишется по
// каждому действию с вкладками (см. handlers::persist).
pub use handlers::persist::{on_read_result, on_write_result};

/// Ресурс открыт. Топиков два — библиотека отдаёт скачанный файл, провайдер
/// открывает ещё не скачанный, — но сообщение одно, так что и обработчик
/// один; чей это ответ — превью или растра наложения, — говорит таблица
/// маршрутов, а не содержимое.
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    if handlers::preview::on_resource_opened(state, &opened) { return; }
    if handlers::overlay::on_opened(state, &opened) { return; }
    veldsdk::resource::discard("on_open_result", opened);
}
pub use handlers::globe::on_probed;
// Ход добычи тайлов наложений: глобус рассказывает, сколько снимку ещё ехать, —
// показывают это списки, у которых есть его строка.
pub use handlers::overlay::on_overlay_progress;

/// Продукт каталога по ключу хранилища. Ждут его двое — показ снимка на шаре и
/// контур отмеченного, — и по содержимому их не различить: продукт в обоих
/// случаях один и тот же. Чей это ответ, говорит таблица маршрутов.
pub fn on_locate_result(
    state: &mut State,
    response: crate::proto::data_provider::LocateResponse,
) {
    let Some(asked) = state.locates.take(&veldsdk::correlation()) else { return };
    match asked {
        state::Locate::Outline(key) => handlers::outline::located(state, key, response),
        // Один ход к каталогу отвечает обоим: показать снимок — это и очертить
        // его (см. `overlay::on_toggle_pressed`), и второй такой же запрос ради
        // той же геометрии был бы лишним.
        state::Locate::Overlay(key) => {
            handlers::outline::located(state, key.clone(), response.clone());
            handlers::overlay::on_located(state, &key, response);
        }
    }
}

/// Растры продукта. Ждут их двое — вкладка превью и собираемое наложение, —
/// и чей это ответ, говорит таблица маршрутов, а не содержимое (то же, что у
/// `on_open_result`).
pub fn on_imagery_result(
    state: &mut State,
    response: crate::proto::data_provider::ImageryResponse,
) {
    if handlers::preview::on_imagery_result(state, &response) { return; }
    handlers::overlay::on_imagery_result(state, response);
}
pub use handlers::search::on_search_result;
pub use handlers::browse::on_list_path_result;
pub use handlers::window::on_window_resized;
