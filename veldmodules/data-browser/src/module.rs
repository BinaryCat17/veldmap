pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;

// -- Types --
pub use handlers::Config;
pub use state::State;

// -- Init --
pub fn init(config: Config) -> anyhow::Result<State> {
    State::new(config)
}

// -- View (Elm-цикл: вызывается сгенерированным раннером после каждого сообщения) --
pub use view::build_root as view;

// -- Input handlers --
pub use handlers::nav::{on_input_nav_browse, on_input_nav_search, on_input_nav_downloaded};
pub use handlers::browse::{on_input_browse, on_input_browse_up};
pub use handlers::search::{on_input_search, on_input_search_input};
pub use handlers::download::{on_input_download_pressed, on_input_view_pressed};

// -- Sub handlers --
pub use handlers::download::{on_sub_download_started, on_sub_download_progress, on_sub_downloaded};
pub use handlers::search::on_sub_search_result;
pub use handlers::browse::on_sub_list_path_result;
pub use handlers::nav::on_sub_list_result;
pub use handlers::window::on_sub_window_resized;
