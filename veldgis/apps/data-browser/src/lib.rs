mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;
mod handlers;

use veldsdk::define_iced_module;
use veldsdk::iced::IcedSettings;
use iced_core::image::Handle;
use veldmap_gis_api::dataprovider::DataProduct;
use crate::common::BrowserItem;

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

pub struct LocalState {
    pub view_mode: crate::app::ViewMode,
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
    SwitchMode(crate::app::ViewMode),
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

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<(LocalState, IcedSettings)> {
    let state = LocalState {
        view_mode: app::ViewMode::Search,
        status_message: "VeldMap Data Browser Ready".to_string(),
        error_message: None,
        search_state: search::SearchState::default(),
        search_results: Vec::new(),
        download_progress: None,
        current_image: None,
        downloaded_state: downloaded::DownloadedState::default(),
        token_stack: Vec::new(),
        next_token: None,
        current_browse_path: String::new(),
        selected_product: None,
        product_files: Vec::new(),
        browse_items: Vec::new(),
        local_files: Vec::new(),
    };
    
    let settings = IcedSettings {
        default_font: iced_core::Font::with_name("VeldMap"),
        fonts: vec![
            ("DejaVuSans", common::DEJAVU_FONT_DATA),
            ("NotoColorEmoji", common::EMOJI_FONT_DATA),
        ],
    };
    Ok((state, settings))
}

define_iced_module! {
    config: LocalConfig,
    state: LocalState,
    message: Message,
    init: module_init,
    view: app::view,
    handlers: {
        Message::SwitchMode(_) => handlers::handle_switch_mode,
        Message::SearchInputChanged(_) => handlers::handle_search_input,
        Message::SearchFilterTypeChanged(_) => handlers::handle_search_filter,
        Message::SearchPressed => handlers::handle_search_press,
        Message::ClearError => handlers::handle_clear_error,
        Message::ProductSelected(_) => handlers::handle_product_selected,
        Message::BackToList => handlers::handle_back_to_list,
        Message::BrowsePath(_) => handlers::handle_browse_path,
        Message::BrowseUp => handlers::handle_browse_up,
        Message::LocalSearchChanged(_) => handlers::handle_local_search,
        Message::LocalFilterChanged(_) => handlers::handle_local_filter,
        Message::DownloadFile(_) => handlers::handle_download,
        Message::DeleteLocalFile(_) => handlers::handle_delete,
        Message::ViewFile(_) => handlers::handle_view,
        Message::ClosePreview => handlers::handle_close_preview,
    }
}