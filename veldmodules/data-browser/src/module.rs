pub mod handlers;
pub mod message;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;

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

/// Библиотеку спрашиваем один раз, на первом же событии: до ответа мы не знаем,
/// что уже скачано, а сама она рассылает только изменения.
///
/// Первым приходит app/on_window_resized — раннер объявляет размер окна, а
/// готовность следом (см. runners/desktop, `announce`). К этому моменту все
/// плагины уже загружены и подписаны, так что запрос доедет до библиотеки.
///
/// Флаг снаружи State: hook_event получает его по ссылке, да и относится это к
/// жизни модуля, а не к тому, что он показывает.
static LIBRARY_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn hook_event(state: &State) {
    if !LIBRARY_REQUESTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        handlers::nav::request_library();
    }
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
        Msg::NavBrowse => handlers::nav::on_nav_browse(state),
        Msg::NavSearch => handlers::nav::on_nav_search(state),
        Msg::NavDownloaded => handlers::nav::on_nav_downloaded(state),
        Msg::TabSelect(id) => handlers::nav::on_tab_select(state, id),
        Msg::TabClose(id) => handlers::nav::on_tab_close(state, id),
        Msg::Browse(path) => handlers::browse::on_browse(state, path),
        Msg::BrowseUp => handlers::browse::on_browse_up(state),
        Msg::Search => handlers::search::on_search(state),
        Msg::SearchInput(query) => handlers::search::on_search_input(state, query),
        Msg::Download(identifier) => handlers::library::on_download_pressed(state, identifier),
        Msg::ViewLocal(name) => handlers::preview::on_view_local_pressed(state, name),
        Msg::ViewRemote(identifier) => handlers::preview::on_view_remote_pressed(state, identifier),
        Msg::Delete(name) => handlers::library::on_delete_pressed(state, name),
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
