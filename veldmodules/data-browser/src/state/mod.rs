pub mod types;
pub use types::*;

pub struct State {
    pub current_screen: types::Screen,
    pub search: search::SearchState,
    pub browse: browse::BrowseState,
    pub library: library::LibraryState,
    pub preview: preview::PreviewState,
    pub global: types::GlobalState,
    /// Render-таргет нашего окна: аллоцируется в ответ на app/window_resized
    /// и делегируется рендереру (см. handlers::window).
    pub window_surface: Option<u64>,
    /// Размер окна в физических пикселях (app/window_resized). Нужен как
    /// потолок для превью: рисовать картинку крупнее окна незачем.
    pub window: (u32, u32),
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        // Стартовый экран — из конфига; умолчание и поведение при неизвестном
        // значении — Search (экран по умолчанию всегда существует).
        let current_screen = match config.initial_screen.as_deref() {
            None | Some("search") => types::Screen::Search,
            Some("browse") => types::Screen::Browse,
            Some("downloaded") => types::Screen::Downloaded,
            Some("preview") => types::Screen::Preview,
            Some(other) => {
                veldsdk::log::warn!(target: "system", "unknown initial_screen '{}', falling back to Search", other);
                types::Screen::Search
            }
        };
        Ok(Self {
            current_screen,
            search: search::SearchState::default(),
            browse: browse::BrowseState::default(),
            library: library::LibraryState::default(),
            preview: preview::PreviewState::default(),
            window_surface: None,
            window: (0, 0),
            global: types::GlobalState {
                status_message: "VeldMap Data Browser".to_string(),
                error_message: None,
            },
        })
    }
}

pub mod search;
pub mod browse;
pub mod library;
pub mod preview;
