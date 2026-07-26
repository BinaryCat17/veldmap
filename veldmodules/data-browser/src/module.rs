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

pub fn hook_event(state: &State) {
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(crate::SERVICE_NAME, root, &mut LAST_UI_HASH.lock().unwrap());
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
        ON_DOWNLOAD_PRESSED => handlers::download::on_download_pressed(state, event),
        ON_VIEW_PRESSED => handlers::download::on_view_pressed(state, event),
        ON_DELETE_PRESSED => handlers::download::on_delete_pressed(state, event),
        other => veldsdk::log::warn!(target: "handlers", "[data-browser] unknown UI method: {}", other),
    }
}

// -- Sub handlers --
pub use handlers::download::{on_download_started, on_download_progress, on_downloaded, on_delete_result, on_write_result, on_read_result, on_load_result};
pub use handlers::search::on_search_result;
pub use handlers::browse::on_list_path_result;
pub use handlers::nav::on_list_result;
pub use handlers::window::on_window_resized;
