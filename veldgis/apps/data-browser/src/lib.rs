mod view;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;
mod handlers;

use veldsdk::define_iced_module;
use iced_core::image::Handle;
use veldmap_gis_api::dataprovider::DataProduct;
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
    pub current_image: Option<Handle>,
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
    ClearError,
    ProductSelected(DataProduct),
    BackToList,
    BrowsePath(String),
    BrowseUp,
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
    DownloadFile(String),
    DeleteLocalFile(String),
    ViewFile(String),
    ClosePreview,
}

define_iced_module! {
    config: LocalConfig,
    state: LocalState,
    message: Message,
    init: handlers::module_init,
    view: view::view,
    handlers: {
        SwitchMode(mode) => async handlers::handle_switch_mode;
        SearchInputChanged(query) => async handlers::handle_search_input;
        SearchFilterTypeChanged(filter) => async handlers::handle_search_filter;
        SearchPressed => async handlers::handle_search_press;
        ClearError => async handlers::handle_clear_error;
        ProductSelected(product) => async handlers::handle_product_selected;
        BackToList => async handlers::handle_back_to_list;
        BrowsePath(path) => async handlers::handle_browse_path;
        BrowseUp => async handlers::handle_browse_up;
        LocalSearchChanged(query) => async handlers::handle_local_search;
        LocalFilterChanged(filter) => async handlers::handle_local_filter;
        DownloadFile(path) => async handlers::handle_download;
        DeleteLocalFile(path) => async handlers::handle_delete;
        ViewFile(path) => async handlers::handle_view;
        ClosePreview => async handlers::handle_close_preview;
    }
}
