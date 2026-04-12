pub mod types;
pub use types::*;

pub struct State {
    pub current_screen: types::Screen,
    pub search: search::SearchState,
    pub browse: browse::BrowseState,
    pub downloaded: downloaded::DownloadedState,
    pub preview: preview::PreviewState,
    pub global: types::GlobalState,
}

impl State {
    pub fn new(_config: crate::handlers::Config) -> anyhow::Result<Self> {
        Ok(Self {
            current_screen: types::Screen::Search,
            search: search::SearchState::default(),
            browse: browse::BrowseState::default(),
            downloaded: downloaded::DownloadedState::default(),
            preview: preview::PreviewState::default(),
            global: types::GlobalState {
                status_message: "VeldMap Data Browser".to_string(),
                error_message: None,
                task_manager: crate::components::task_manager::TaskManager::default(),
                has_rendered: false,
            },
        })
    }
}

pub mod search;
pub mod browse;
pub mod downloaded;
pub mod preview;
