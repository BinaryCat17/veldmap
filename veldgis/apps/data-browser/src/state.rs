//! state.rs — чистое иерархическое состояние приложения
//! Исправлен Default для Screen (ручная реализация, т.к. варианты с данными)

use veldsdk::core::task::TaskStatus;
use veldmap_gis_api::dataprovider::{SearchResponse, ListPathResponse, DownloadResponse};
use veldsdk::rpc::core::ResourceHandle;

use crate::common::BrowserItem;

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
    Search(crate::search::SearchState),
    Browse(crate::browse::BrowseState),
    Downloaded(crate::downloaded::DownloadedState),
    Preview(crate::preview::PreviewState),
}

impl Default for Screen {
    fn default() -> Self {
        Screen::Search(crate::search::SearchState::default())
    }
}

/// Главное состояние приложения
#[derive(Default)]
pub struct AppState {
    pub screen: Screen,
    pub global: GlobalState,
}
