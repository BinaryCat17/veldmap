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

// -- Input handlers --
pub use handlers::nav::{on_nav_browse, on_nav_search, on_nav_downloaded};
pub use handlers::browse::{on_browse, on_browse_up};
pub use handlers::search::{on_search, on_search_input};
pub use handlers::download::{on_download_pressed, on_view_pressed};

// -- Sub handlers --
pub use handlers::download::{on_download_started, on_download_progress, on_downloaded};
pub use handlers::search::on_search_result;
pub use handlers::browse::on_list_path_result;
pub use handlers::nav::on_list_result;
pub use handlers::window::on_window_resized;
