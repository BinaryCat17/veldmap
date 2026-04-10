//! app/state.rs — главное состояние приложения

use crate::common::BrowserItem;
use crate::service::task_manager::TaskManager;
use crate::screens::{SearchState, BrowseState, DownloadedState, PreviewState};

/// Глобальное состояние
#[derive(Default, Clone)]
pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
    pub local_files: Vec<BrowserItem>,

    // Единый менеджер задач
    pub task_manager: TaskManager,
}

/// Все экраны приложения
#[derive(Clone)]
pub enum Screen {
    Search(SearchState),
    Browse(BrowseState),
    Downloaded(DownloadedState),
    Preview(PreviewState),
}

impl Default for Screen {
    fn default() -> Self {
        Screen::Search(SearchState::default())
    }
}

/// Главное состояние
#[derive(Default, Clone)]
pub struct AppState {
    pub screen: Screen,
    pub global: GlobalState,
}
