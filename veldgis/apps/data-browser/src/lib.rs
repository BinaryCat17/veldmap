mod view;
mod common;
mod search;
mod browse;
mod downloaded;
mod preview;
mod handlers;

use veldsdk::define_iced_module;
use veldsdk::iced_core::image::Handle;
use veldmap_gis_api::dataprovider::{DataProduct, SearchResponse, ListPathResponse};
use crate::common::{BrowserItem, ViewMode};

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

pub struct LocalState {
    pub view_mode: ViewMode,
    pub status_message: String,
    pub error_message: Option<String>,
    pub search_state: search::SearchState,
    pub search_results: Vec<DataProduct>,
    pub download_progress: Option<f32>,
    pub active_download_task: Option<String>,
    pub active_image_task: Option<String>,
    pub current_image: Option<Handle>,
    pub current_gpu_image: Option<veldsdk::rpc::core::ResourceHandle>,
    pub downloaded_state: downloaded::DownloadedState,
    pub token_stack: Vec<String>,
    pub next_token: Option<String>,
    pub current_browse_path: String,
    pub selected_product: Option<String>,
    pub product_files: Vec<BrowserItem>,
    pub browse_items: Vec<BrowserItem>,
    pub local_files: Vec<BrowserItem>,
}

#[derive(Debug, Clone)]
pub enum Message {
    SwitchMode(ViewMode),
    SearchInputChanged(String),
    SearchFilterTypeChanged(search::SearchFilterType),
    SearchPressed,
    SearchResult(Result<SearchResponse, String>),
    ClearError,
    ProductSelected(DataProduct),
    ProductFilesLoaded(Result<ListPathResponse, String>),
    BackToList,
    BrowsePath(String),
    BrowsePathLoaded(Result<(String, ListPathResponse), String>),
    BrowseUp,
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
    DownloadFile(String),
    DownloadStarted(Result<String, String>),
    UpdateDownloadProgress,
    CancelDownload,
    DeleteLocalFile(String),
    ViewFile(String),
    PreviewLoaded(Result<Handle, String>),
    ImageStatusUpdated,
    ClosePreview,
}

define_iced_module! {
    config: LocalConfig,
    state: LocalState,
    message: Message,
    init: handlers::module_init,
    view: view::view,
    handlers: {
        SwitchMode(mode) => handlers::handle_switch_mode;
        SearchInputChanged(query) => handlers::handle_search_input;
        SearchFilterTypeChanged(filter) => handlers::handle_search_filter;
        SearchPressed => handlers::handle_search_press;
        SearchResult(res) => handlers::handle_search_result;
        ClearError => handlers::handle_clear_error;
        ProductSelected(product) => handlers::handle_product_selected;
        ProductFilesLoaded(res) => handlers::handle_product_files_loaded;
        BackToList => handlers::handle_back_to_list;
        BrowsePath(path) => handlers::handle_browse_path;
        BrowsePathLoaded(res) => handlers::handle_browse_path_loaded;
        BrowseUp => handlers::handle_browse_up;
        LocalSearchChanged(query) => handlers::handle_local_search;
        LocalFilterChanged(filter) => handlers::handle_local_filter;
        DownloadFile(path) => handlers::handle_download;
        DownloadStarted(res) => handlers::handle_download_started;
        UpdateDownloadProgress => handlers::handle_update_progress;
        CancelDownload => handlers::handle_cancel_download;
        DeleteLocalFile(path) => handlers::handle_delete;
        ViewFile(path) => handlers::handle_view;
        PreviewLoaded(res) => handlers::handle_preview_loaded;
        ImageStatusUpdated => handlers::handle_image_status;
        ClosePreview => handlers::handle_close_preview;
    }
}
