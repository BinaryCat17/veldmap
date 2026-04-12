//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod download;
pub mod nav;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_screen: Option<String>,
}

/// Frame handler - единственное место где вызывается render.
/// Вызывается каждый кадр, рендерит текущее состояние.
pub fn on_frame(state: &mut crate::state::State, frame: veld_ui::proto::FrameEvent) {
    let root = crate::view::build_root(state);
    veld_ui::render::render("data-browser", root, &mut state.last_layout, frame.width, frame.height);
}
