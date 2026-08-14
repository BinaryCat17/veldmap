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
// сообщения). Пересобирает view и шлёт в ui-service; неизменный layout не
// уходит по сети — дедуп по хэшу внутри render(). --
static LAST_UI_HASH: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

pub fn hook_event(state: &State) {
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(
        root,
        &mut LAST_UI_HASH.lock().unwrap(),
        crate::calls::ui_service::on_set_view,
    );
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

    match message {
        Msg::TabSelect(id) => handlers::nav::on_tab_select(state, id),
        Msg::TabClose(id) => handlers::nav::on_tab_close(state, id),
        Msg::TabMenu(half) => handlers::nav::on_tab_menu(state, half),
        Msg::TabOptions(id) => handlers::nav::on_tab_options(state, id),
        Msg::NewTab(half, kind) => handlers::nav::on_new_tab(state, half, kind),
        Msg::TabMove(id) => handlers::nav::on_tab_move(state, id),
        Msg::TabSplit(id, placement) => handlers::nav::on_tab_split(state, id, placement),
        Msg::TabUnsplit => handlers::nav::on_tab_unsplit(state),
        Msg::Download(identifier, product) => {
            handlers::library::on_download_pressed(state, identifier, product)
        }
        Msg::Delete(name) => handlers::library::on_delete_pressed(state, name),
        Msg::DeleteSnapshot(product) => handlers::library::on_delete_snapshot(state, product),
        Msg::Reveal(name) => handlers::library::on_reveal_pressed(state, name),
        // «Снять с шара» — про всё, что на нём лежит: и растры, и контуры.
        // Порознь их не снять ничем, а разделять их пользователю не по чему —
        // он видит один шар.
        Msg::GlobeClear => {
            handlers::overlay::clear_all(state);
            handlers::search::clear_outlines(state);
        }
        Msg::OverlayOpacity(key, value) => handlers::overlay::set_opacity(state, &key, value),
        Msg::OverlayHidden(key, hidden) => handlers::overlay::set_hidden(state, &key, hidden),
        Msg::OverlayRemove(key) => handlers::overlay::remove(state, &key),
        Msg::OverlayShift(key, shift) => handlers::overlay::shift(state, &key, shift),
        Msg::OverlayHideAll(hidden) => handlers::overlay::hide_all(state, hidden),
        Msg::In(view, message) => on_view_message(state, view, message),
    }
}

/// Сообщение из тела вкладки. Вид назвал себя сам (см. `Msg::In`), поэтому
/// адресат здесь известен точно — и обработчику не приходится гадать, какая из
/// половин экрана была под рукой.
fn on_view_message(state: &mut State, view: crate::module::state::ViewId, message: ViewMsg) {
    match message {
        ViewMsg::OpenMenu(menu) => handlers::listing::on_menu(state, view, menu),
        ViewMsg::Filter(filter) => handlers::listing::on_filter(state, view, filter),
        ViewMsg::Group(grouping) => handlers::listing::on_group(state, view, grouping),
        ViewMsg::Sort(sorting) => handlers::listing::on_sort(state, view, sorting),
        ViewMsg::Query(query) => handlers::listing::on_query(state, view, query),
        ViewMsg::Page(page) => handlers::listing::on_page(state, view, page),
        ViewMsg::Expand(key) => handlers::listing::on_expand(state, view, key),
        ViewMsg::SearchQuery(query) => handlers::search::on_query(state, view, query),
        ViewMsg::SearchMission(mission) => handlers::search::on_mission(state, view, mission),
        ViewMsg::SearchPeriod(period) => handlers::search::on_period(state, view, period),
        ViewMsg::SearchFrom(value) => handlers::search::on_from(state, view, value),
        ViewMsg::SearchTo(value) => handlers::search::on_to(state, view, value),
        ViewMsg::SearchCloud(cloud) => handlers::search::on_cloud(state, view, cloud),
        ViewMsg::RunSearch => handlers::search::run(state, view),
        ViewMsg::Enter(path) => handlers::browse::on_enter(state, view, path),
        ViewMsg::Up => handlers::browse::on_up(state, view),
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
        ViewMsg::GlobeResized(size) => handlers::globe::on_resized(state, view, size),
        ViewMsg::GlobePointer(event) => handlers::globe::on_pointer(state, view, event),
        ViewMsg::GlobeShow(identifier) => handlers::overlay::on_show_pressed(state, view, identifier),
    }
}

// -- Sub handlers --
pub use handlers::library::on_state;
pub use handlers::preview::on_view_state;

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
pub use handlers::overlay::on_locate_result;

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
