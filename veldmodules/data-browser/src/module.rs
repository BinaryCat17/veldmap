pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;

// -- Types --
pub use handlers::Config;
pub use state::State;

// -- Init --
pub fn hook_init(config: Config) -> anyhow::Result<State> {
    State::new(config)
}

// -- Event hook (Elm-цикл: вызывается сгенерированным раннером после каждого
// сообщения). Пересобирает view и шлёт в ui-service; неизменный layout не
// уходит по сети — дедуп по хэшу внутри render(). --
static LAST_UI_HASH: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// Первый вызов hook_event — это app/on_ready: все сервисы подняты. Тогда же
/// и спрашиваем библиотеку, иначе до первого перехода между экранами мы не
/// знали бы, что уже скачано. Флаг снаружи State: hook_event получает его по
/// ссылке, да и относится это к жизни модуля, а не к тому, что он показывает.
static LIBRARY_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn hook_event(state: &State) {
    if !LIBRARY_REQUESTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        handlers::nav::request_library();
    }
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(
        crate::SERVICE_NAME,
        root,
        &mut LAST_UI_HASH.lock().unwrap(),
        crate::calls::ui_service::on_set_view,
    );
}

// -- UI-события (ui-service/on_ui_event, адресовано нам через target) --
// Один топик, много методов: ui-service возвращает нажатую кнопку эхом в
// UiEventResponse.method, дальше это внутренняя проводка модуля, а не часть
// bus-контракта — см. handlers::ui_methods.
pub fn on_ui_event(state: &mut State, event: crate::proto::ui_service::proto::UiEventResponse) {
    use handlers::ui_methods::*;
    match event.method.as_str() {
        ON_NAV_BROWSE => handlers::nav::on_nav_browse(state, event),
        ON_NAV_SEARCH => handlers::nav::on_nav_search(state, event),
        ON_NAV_DOWNLOADED => handlers::nav::on_nav_downloaded(state, event),
        ON_BROWSE => handlers::browse::on_browse(state, event),
        ON_BROWSE_UP => handlers::browse::on_browse_up(state, event),
        ON_SEARCH => handlers::search::on_search(state, event),
        ON_SEARCH_INPUT => handlers::search::on_search_input(state, event),
        ON_DOWNLOAD_PRESSED => handlers::library::on_download_pressed(state, event),
        ON_VIEW_LOCAL_PRESSED => handlers::preview::on_view_local_pressed(state, event),
        ON_VIEW_REMOTE_PRESSED => handlers::preview::on_view_remote_pressed(state, event),
        ON_DELETE_PRESSED => handlers::library::on_delete_pressed(state, event),
        other => veldsdk::log::warn!(target: "handlers", "unknown UI method: {}", other),
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
