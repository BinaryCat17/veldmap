//! app/state.rs — главное состояние приложения

use veldsdk::core::task::TaskStatus;
use veldmap_api::dataprovider::{SearchResponse, ListPathResponse, DownloadResponse};
use veldsdk::rpc::core::ResourceHandle;

use crate::common::BrowserItem;
use crate::task_manager::TaskManager;
use crate::screens::{SearchState, BrowseState, DownloadedState, PreviewState};

/// Глобальное состояние
#[derive(Default, Clone)]
pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
    pub local_files: Vec<BrowserItem>,

    // Единый менеджер задач вместо разрозненных TaskStatus
    pub task_manager: TaskManager,
    
    // Текущая загрузка (для обновления TaskManager)
    pub current_download: Option<String>, // s3_key
    
    // Пока оставляем TaskStatus для совместимости с существующим кодом
    // TODO: постепенно перевести всё на task_manager
    pub search_task: TaskStatus<SearchResponse>,
    pub browse_task: TaskStatus<ListPathResponse>,
    pub download_task: TaskStatus<DownloadResponse>,
    pub image_task: TaskStatus<ResourceHandle>,
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
