pub mod handlers;
pub mod message;
pub mod state;
pub mod view;
pub mod components;
pub mod theme;

// -- Types --
pub use handlers::Config;
pub use message::Msg;
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
    let Some(message) = Msg::decode(&event.method, &event.value) else {
        veldsdk::log::warn!(target: "handlers", "непонятное сообщение разметки: '{}' = '{}'", event.method, event.value);
        return;
    };

    match message {
        Msg::TabSelect(id) => handlers::nav::on_tab_select(state, id),
        Msg::TabClose(id) => handlers::nav::on_tab_close(state, id),
        Msg::TabMenu(open) => handlers::nav::on_tab_menu(state, open),
        Msg::NewBrowse => handlers::nav::on_new_browse(state),
        Msg::NewSearch => handlers::nav::on_new_search(state),
        Msg::NewDownloaded => handlers::nav::on_new_downloaded(state),
        Msg::OpenMenu(menu) => handlers::listing::on_menu(state, menu),
        Msg::Filter(filter) => handlers::listing::on_filter(state, filter),
        Msg::Group(grouping) => handlers::listing::on_group(state, grouping),
        Msg::Sort(sorting) => handlers::listing::on_sort(state, sorting),
        Msg::Query(query) => handlers::listing::on_query(state, query),
        Msg::Page(page) => handlers::listing::on_page(state, page),
        Msg::Enter(path) => handlers::browse::on_enter(state, path),
        Msg::Up => handlers::browse::on_up(state),
        Msg::Download(identifier) => handlers::library::on_download_pressed(state, identifier),
        Msg::Delete(name) => handlers::library::on_delete_pressed(state, name),
        Msg::Preview(name) => handlers::preview::on_view_local_pressed(state, name),
        Msg::PreviewRemote(identifier) => handlers::preview::on_view_remote_pressed(state, identifier),
        Msg::Zoom(zoom) => handlers::preview::on_zoom(state, zoom),
    }
}

// -- Sub handlers --
pub use handlers::library::on_state;
pub use handlers::preview::on_load_result;

/// Ресурс открыт. Топиков два — библиотека отдаёт скачанный файл, провайдер
/// открывает ещё не скачанный, — но сообщение одно и потребитель один, так
/// что и обработчик один: дальше превью безразлично, откуда взялись байты.
///
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    if handlers::preview::on_resource_opened(state, &opened) { return; }
    veldsdk::resource::discard("on_open_result", opened);
}
pub use handlers::search::on_search_result;
pub use handlers::browse::on_list_path_result;
pub use handlers::window::on_window_resized;
