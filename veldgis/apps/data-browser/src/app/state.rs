//! app/state.rs — главное состояние приложения

use veldsdk::core::task::TaskStatus;
use veldmap_api::dataprovider::{SearchResponse, ListPathResponse, DownloadResponse};
use veldsdk::rpc::core::ResourceHandle;

use crate::common::BrowserItem;
use crate::screens::{SearchState, BrowseState, DownloadedState, PreviewState};

/// Глобальное состояние
#[derive(Default)]
pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
    pub downloading_key: Option<String>,
    pub local_files: Vec<BrowserItem>,

    pub search_task: TaskStatus<SearchResponse>,
    pub browse_task: TaskStatus<ListPathResponse>,
    pub download_task: TaskStatus<DownloadResponse>,
    pub image_task: TaskStatus<ResourceHandle>,
}

/// Все экраны приложения
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
#[derive(Default)]
pub struct AppState {
    pub screen: Screen,
    pub global: GlobalState,
}
