use crate::{LocalState, utils, common};
use crate::view::ViewMode;
use crate::common::BrowserItem;
use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, ListPathRequest, ListPathResponse, DownloadRequest, DownloadResponse, SearchFilter, DataProduct};
use prost::Message as ProstMessage;
use iced_core::image::Handle;
use veldsdk::iced::IcedSettings;
use crate::LocalConfig;

pub fn module_init(_cfg: LocalConfig) -> anyhow::Result<(LocalState, IcedSettings)> {
    let state = LocalState {
        view_mode: ViewMode::Search,
        status_message: "VeldMap Data Browser Ready".to_string(),
        error_message: None,
        search_state: crate::search::SearchState::default(),
        search_results: Vec::new(),
        download_progress: None,
        current_image: None,
        downloaded_state: crate::downloaded::DownloadedState::default(),
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

pub fn handle_switch_mode(state: &mut LocalState, mode: ViewMode) {
    state.view_mode = mode;
    state.current_image = None;
    state.download_progress = None;
    state.selected_product = None;
    if state.view_mode == ViewMode::Browse && state.browse_items.is_empty() {
        perform_browse(state, String::new());
    } else if state.view_mode == ViewMode::Downloaded {
        refresh_local_files(state);
    }
}

pub fn handle_search_input(state: &mut LocalState, q: String) {
    state.search_state.query = q;
}

pub fn handle_search_filter(state: &mut LocalState, ft: crate::search::SearchFilterType) {
    state.search_state.filter_type = ft;
}

pub fn handle_search_press(state: &mut LocalState) {
    state.status_message = "Searching CDSE...".to_string();
    let mut filters = Vec::new();
    match state.search_state.filter_type {
        crate::search::SearchFilterType::GridId => filters.push(SearchFilter { name: "gridId".into(), value: state.search_state.query.clone() }),
        crate::search::SearchFilterType::Collection => filters.push(SearchFilter { name: "Collection".into(), value: state.search_state.query.clone() }),
        _ => {}
    }
    let q = if state.search_state.filter_type == crate::search::SearchFilterType::General { state.search_state.query.clone() } else { String::new() };
    
    let req = SearchRequest { query: q, filters };
    match veldsdk::rpc::host::call_service("data-provider", "search", req.encode_to_vec()) {
        Ok(res_bytes) => {
            if let Ok(response) = SearchResponse::decode(&res_bytes[..]) {
                if !response.error.is_empty() {
                    state.error_message = Some(format!("Search API Error: {}", response.error));
                } else {
                    state.search_results = response.products;
                    state.status_message = format!("Found {} results", state.search_results.len());
                }
            }
        }
        Err(e) => { state.error_message = Some(format!("Search Error: {}", e)); }
    }
}

pub fn handle_product_selected(state: &mut LocalState, prod: DataProduct) {
    state.status_message = format!("Loading files for {}...", prod.name);
    let req = ListPathRequest { path: prod.path.clone(), token: String::new() };
    match veldsdk::rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
        Ok(res_bytes) => {
            if let Ok(response) = ListPathResponse::decode(&res_bytes[..]) {
                state.product_files = response.items.into_iter().map(|s3_key| {
                    let name = s3_key.split('/').last().unwrap_or(&s3_key).to_string();
                    let is_folder = s3_key.ends_with('/');
                    BrowserItem { s3_key, name, is_folder, exists_locally: false }
                }).collect();
                state.selected_product = Some(prod.name.clone());
                state.status_message = format!("Loaded {} items", state.product_files.len());
            }
        }
        Err(e) => { state.error_message = Some(format!("List Error: {}", e)); }
    }
}

pub fn handle_back_to_list(state: &mut LocalState) {
    state.selected_product = None;
}

pub fn handle_browse_path(state: &mut LocalState, path: String) {
    perform_browse(state, path);
}

pub fn handle_browse_up(state: &mut LocalState) {
    let current = state.current_browse_path.trim_end_matches('/');
    if let Some(idx) = current.rfind('/') {
        let parent = current[..=idx].to_string();
        perform_browse(state, parent);
    } else {
        perform_browse(state, String::new());
    }
}

pub fn handle_download(state: &mut LocalState, s3_key: String) {
    let filename = s3_key.split('/').last().unwrap_or("file");
    let dest = format!("data/dem/source/{}", filename);
    let req = DownloadRequest { identifier: s3_key, destination: dest };
    match veldsdk::rpc::host::call_service("data-provider", "download", req.encode_to_vec()) {
        Ok(res_bytes) => {
            if let Ok(response) = DownloadResponse::decode(&res_bytes[..]) {
                if response.success {
                    state.status_message = "Download started".into();
                } else {
                    state.error_message = Some(format!("Download failed: {}", response.error));
                }
            }
        }
        Err(e) => { state.error_message = Some(format!("Download Error: {}", e)); }
    }
}

pub fn handle_delete(state: &mut LocalState, path: String) {
    match veldsdk::core::fs_delete(&path) {
        Ok(_) => {
            state.status_message = format!("Deleted {}", path);
            refresh_local_files(state);
        }
        Err(e) => {
            state.error_message = Some(format!("Failed to delete {}: {}", path, e));
        }
    }
}

pub fn handle_view(state: &mut LocalState, path: String) {
    state.status_message = format!("Loading preview for {}...", path);
    match veldsdk::core::fs_read(&path) {
        Ok(data) => {
            if path.ends_with(".jpg") || path.ends_with(".png") {
                state.current_image = Some(Handle::from_bytes(data));
                state.status_message = "Preview loaded".into();
            } else if path.ends_with(".tif") || path.ends_with(".tiff") {
                match utils::decode_tiff(&data) {
                    Ok((w, h, rgba)) => {
                        state.current_image = Some(Handle::from_rgba(w, h, rgba));
                        state.status_message = "TIFF Preview loaded".into();
                    }
                    Err(e) => {
                        state.error_message = Some(format!("Failed to decode TIFF: {}", e));
                    }
                }
            } else {
                state.error_message = Some("Unsupported file format for preview".into());
            }
        }
        Err(e) => {
            state.error_message = Some(format!("Failed to read file: {}", e));
        }
    }
}

pub fn handle_clear_error(state: &mut LocalState) {
    state.error_message = None;
}

pub fn handle_local_search(state: &mut LocalState, q: String) {
    state.downloaded_state.search_query = q;
}

pub fn handle_local_filter(state: &mut LocalState, f: crate::downloaded::FileFilter) {
    state.downloaded_state.filter = f;
}

pub fn handle_close_preview(state: &mut LocalState) {
    state.current_image = None;
}

// Helper functions

fn perform_browse(state: &mut LocalState, path: String) {
    state.status_message = format!("Listing /{}...", path);
    let req = ListPathRequest { path: path.clone(), token: String::new() };
    match veldsdk::rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
        Ok(res_bytes) => {
            if let Ok(response) = ListPathResponse::decode(&res_bytes[..]) {
                if !response.error.is_empty() {
                    state.error_message = Some(format!("Browse API Error: {}", response.error));
                    state.status_message = "Browse failed".to_string();
                } else {
                    state.browse_items = response.items.into_iter().map(|s3_key| {
                        let is_folder = s3_key.ends_with('/');
                        let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(&s3_key).to_string();
                        BrowserItem { s3_key, name, is_folder, exists_locally: false }
                    }).collect();
                    state.current_browse_path = path;
                    state.next_token = if response.next_token.is_empty() { None } else { Some(response.next_token) };
                    state.status_message = format!("Loaded {} items", state.browse_items.len());
                }
            } else {
                state.error_message = Some("Failed to decode ListPathResponse".into());
            }
        }
        Err(e) => { 
            state.error_message = Some(format!("Browse RPC Error: {}", e)); 
            state.status_message = "Browse failed".to_string();
        }
    }
}

fn refresh_local_files(state: &mut LocalState) {
    state.status_message = "Refreshing local files...".to_string();
    let path = "data/dem/source";
    match veldsdk::core::fs_list(path) {
        Ok(entries) => {
            state.local_files = entries.into_iter().map(|name| {
                BrowserItem {
                    s3_key: format!("{}/{}", path, name),
                    name,
                    is_folder: false,
                    exists_locally: true,
                }
            }).collect();
            state.status_message = format!("Found {} local files", state.local_files.len());
        }
        Err(e) => {
            state.error_message = Some(format!("Failed to list local files: {}", e));
        }
    }
}
