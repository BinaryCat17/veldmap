mod handlers;
mod view;
mod common;
pub mod styles;
mod search;
mod browse;
mod downloaded;

use veld_ui::define_remote_ui_module;
use veldmap_gis_api::dataprovider::{DataProduct, SearchResponse, ListPathResponse, DownloadResponse};
use crate::common::{BrowserItem, ViewMode};

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

use veldsdk::core::task::TaskStatus;

pub struct LocalState {
    pub view_mode: ViewMode,
    pub status_message: String,
    pub error_message: Option<String>,
    pub search_state: search::SearchState,
    
    // Новые типизированные задачи
    pub search_task: TaskStatus<SearchResponse>,
    pub browse_task: TaskStatus<ListPathResponse>,
    pub download_task: TaskStatus<DownloadResponse>,
    pub downloading_key: Option<String>,
    pub image_task: TaskStatus<veldsdk::rpc::core::ResourceHandle>,

    pub search_results: Vec<DataProduct>,
    pub current_gpu_image: Option<veldsdk::rpc::core::ResourceHandle>,
    pub downloaded_state: downloaded::DownloadedState,
    pub token_stack: Vec<String>,
    pub current_page_token: String,
    pub next_token: Option<String>,
    pub current_browse_path: String,
    pub browse_items: Vec<BrowserItem>,
    pub local_files: Vec<BrowserItem>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AppMessage {
    SwitchMode(ViewMode),
    SearchInputChanged(String),
    SearchFilterTypeChanged(search::SearchFilterType),
    SearchPressed,
    SearchUpdate(veldsdk::core::task::TaskUpdate<SearchResponse>),
    ClearError,
    ProductSelected(DataProduct),
    ProductFilesLoaded(Result<ListPathResponse, String>),
    BackToList,
    BrowsePath(String),
    BrowseUpdate(veldsdk::core::task::TaskUpdate<ListPathResponse>),
    NextPage,
    PrevPage,
    BrowseUp,
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
    DownloadFile(String),
    DownloadUpdate(veldsdk::core::task::TaskUpdate<DownloadResponse>),
    CancelDownload,
    DeleteLocalFile(String),
    ViewFile(String),
    ImageUpdate(veldsdk::core::task::TaskUpdate<veldsdk::rpc::core::ResourceHandle>),
    ClosePreview,
}

define_remote_ui_module! {
    config: LocalConfig,
    state: LocalState,
    message: AppMessage,
    init: handlers::module_init,
    view: view::view,
    handlers: {
        SwitchMode(mode) => handlers::handle_switch_mode;
        SearchInputChanged(query) => handlers::handle_search_input;
        SearchFilterTypeChanged(filter) => handlers::handle_search_filter;
        SearchPressed => handlers::handle_search_press;
        SearchUpdate(u) => handlers::handle_search_update;
        ClearError => handlers::handle_clear_error;
        ProductSelected(product) => handlers::handle_product_selected;
        ProductFilesLoaded(res) => handlers::handle_product_files_loaded;
        BackToList => handlers::handle_back_to_list;
        BrowsePath(path) => handlers::handle_browse_path;
        BrowseUpdate(u) => handlers::handle_browse_update;
        NextPage => handlers::handle_next_page;
        PrevPage => handlers::handle_prev_page;
        BrowseUp => handlers::handle_browse_up;
        LocalSearchChanged(query) => handlers::handle_local_search;
        LocalFilterChanged(filter) => handlers::handle_local_filter;
        DownloadFile(path) => handlers::handle_download;
        DownloadUpdate(u) => handlers::handle_download_update;
        CancelDownload => handlers::handle_cancel_download;
        DeleteLocalFile(path) => handlers::handle_delete;
        ViewFile(path) => handlers::handle_view;
        ImageUpdate(u) => handlers::handle_image_update;
        ClosePreview => handlers::handle_close_preview;
    }
}
