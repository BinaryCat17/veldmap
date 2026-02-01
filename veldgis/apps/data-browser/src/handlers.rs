use crate::{LocalState, utils, common, Message};
use crate::common::ViewMode;
use crate::common::BrowserItem;
use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, ListPathRequest, ListPathResponse, DownloadRequest, DownloadResponse, SearchFilter, DataProduct};
use prost::Message as ProstMessage;
use iced_core::image::Handle;
use veldsdk::iced::{IcedSettings, Command};
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

pub fn handle_switch_mode(state: &mut LocalState, mode: ViewMode) -> Command<Message> {
    state.view_mode = mode;
    state.current_image = None;
    state.download_progress = None;
    state.selected_product = None;
    if state.view_mode == ViewMode::Browse && state.browse_items.is_empty() {
        return perform_browse_cmd(String::new());
    } else if state.view_mode == ViewMode::Downloaded {
        refresh_local_files(state);
    }
    Command::none()
}

pub fn handle_search_input(state: &mut LocalState, q: String) -> Command<Message> { 
    state.search_state.query = q; 
    Command::none()
}

pub fn handle_search_filter(state: &mut LocalState, ft: crate::search::SearchFilterType) -> Command<Message> { 
    state.search_state.filter_type = ft; 
    Command::none()
}

pub fn handle_search_press(state: &mut LocalState) -> Command<Message> {
    state.status_message = "Searching CDSE...".to_string();
    
    let mut filters = Vec::new();
    match state.search_state.filter_type {
        crate::search::SearchFilterType::GridId => filters.push(SearchFilter { name: "gridId".into(), value: state.search_state.query.clone() }),
        crate::search::SearchFilterType::Collection => filters.push(SearchFilter { name: "Collection".into(), value: state.search_state.query.clone() }),
        _ => {}
    }
    let q = if state.search_state.filter_type == crate::search::SearchFilterType::General { state.search_state.query.clone() } else { String::new() };
    
    let req = SearchRequest { query: q, filters };
    
    Command::perform(async move {
        // Мы все еще используем yield_now в начале, чтобы хост мог отрисовать "Searching..."
        veldsdk::yield_now().await;
        
        match veldsdk::rpc::host::call_service("data-provider", "search", req.encode_to_vec()) {
            Ok(res_bytes) => {
                match SearchResponse::decode(&res_bytes[..]) {
                    Ok(response) => Message::SearchResult(Ok(response)),
                    Err(e) => Message::SearchResult(Err(format!("Decode Error: {}", e))),
                }
            }
            Err(e) => Message::SearchResult(Err(format!("Search Error: {}", e))),
        }
    })
}

pub fn handle_search_result(state: &mut LocalState, res: Result<SearchResponse, String>) -> Command<Message> {
    match res {
        Ok(response) => {
            if !response.error.is_empty() {
                state.error_message = Some(format!("Search API Error: {}", response.error));
            } else {
                state.search_results = response.products;
                state.status_message = format!("Found {} results", state.search_results.len());
            }
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_product_selected(state: &mut LocalState, prod: DataProduct) -> Command<Message> {
    state.status_message = format!("Loading files for {}...", prod.name);
    let path = prod.path.clone();
    
    Command::perform(async move {
        veldsdk::yield_now().await;
        let req = ListPathRequest { path, token: String::new() };
        match veldsdk::rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
            Ok(res_bytes) => {
                match ListPathResponse::decode(&res_bytes[..]) {
                    Ok(response) => Message::ProductFilesLoaded(Ok(response)),
                    Err(e) => Message::ProductFilesLoaded(Err(format!("Decode Error: {}", e))),
                }
            }
            Err(e) => Message::ProductFilesLoaded(Err(format!("List Error: {}", e))),
        }
    })
}

pub fn handle_product_files_loaded(state: &mut LocalState, res: Result<ListPathResponse, String>) -> Command<Message> {
    match res {
        Ok(response) => {
            let local_files = veldsdk::core::fs_list("data/dem/source").unwrap_or_default();
            state.product_files = response.items.into_iter().map(|s3_key| {
                let name = s3_key.split('/').last().unwrap_or(&s3_key).to_string();
                let is_folder = s3_key.ends_with('/');
                let exists_locally = local_files.contains(&name);
                BrowserItem { s3_key, name, is_folder, exists_locally }
            }).collect();
            state.status_message = format!("Loaded {} items", state.product_files.len());
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_back_to_list(state: &mut LocalState) -> Command<Message> { 
    state.selected_product = None; 
    Command::none()
}

pub fn handle_browse_path(state: &mut LocalState, path: String) -> Command<Message> {
    state.status_message = format!("Listing /{}...", path);
    perform_browse_cmd(path)
}

pub fn handle_browse_path_loaded(state: &mut LocalState, res: Result<(String, ListPathResponse), String>) -> Command<Message> {
    match res {
        Ok((path, response)) => {
            let local_files = veldsdk::core::fs_list("data/dem/source").unwrap_or_default();
            state.browse_items = response.items.into_iter().map(|s3_key| {
                let is_folder = s3_key.ends_with('/');
                let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(&s3_key).to_string();
                let exists_locally = !is_folder && local_files.contains(&name);
                BrowserItem { s3_key, name, is_folder, exists_locally }
            }).collect();
            state.current_browse_path = path;
            state.status_message = format!("Loaded {} items", state.browse_items.len());
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_browse_up(state: &mut LocalState) -> Command<Message> {
    let current = state.current_browse_path.trim_end_matches('/');
    let parent = if let Some(idx) = current.rfind('/') {
        current[..=idx].to_string()
    } else {
        String::new()
    };
    state.status_message = format!("Listing /{}...", parent);
    perform_browse_cmd(parent)
}

pub fn handle_download(state: &mut LocalState, s3_key: String) -> Command<Message> {
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    state.status_message = format!("Downloading {}...", filename);
    
    Command::perform(async move {
        veldsdk::yield_now().await;
        let dest = format!("data/dem/source/{}", filename);
        let req = DownloadRequest { identifier: s3_key, destination: dest };
        match veldsdk::rpc::host::call_service("data-provider", "download", req.encode_to_vec()) {
            Ok(res_bytes) => {
                match DownloadResponse::decode(&res_bytes[..]) {
                    Ok(response) => {
                        if response.success { Message::DownloadFinished(Ok(filename)) }
                        else { Message::DownloadFinished(Err(format!("Download failed: {}", response.error))) }
                    }
                    Err(e) => Message::DownloadFinished(Err(format!("Decode Error: {}", e))),
                }
            }
            Err(e) => Message::DownloadFinished(Err(format!("Download Error: {}", e))),
        }
    })
}

pub fn handle_download_finished(state: &mut LocalState, res: Result<String, String>) -> Command<Message> {
    match res {
        Ok(_) => {
            state.status_message = "Download complete".into();
            if let ViewMode::Browse = state.view_mode {
                return perform_browse_cmd(state.current_browse_path.clone());
            } else if state.selected_product.is_some() {
                let local_files = veldsdk::core::fs_list("data/dem/source").unwrap_or_default();
                for item in &mut state.product_files {
                    if local_files.contains(&item.name) { item.exists_locally = true; }
                }
            }
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_delete(state: &mut LocalState, path: String) -> Command<Message> {
    match veldsdk::core::fs_delete(&path) {
        Ok(_) => { state.status_message = format!("Deleted {}", path); refresh_local_files(state); }
        Err(e) => { state.error_message = Some(format!("Failed to delete {}: {}", path, e)); }
    }
    Command::none()
}

pub fn handle_view(state: &mut LocalState, path: String) -> Command<Message> {
    state.status_message = format!("Loading preview for {}...", path);
    Command::perform(async move {
        veldsdk::yield_now().await;
        match veldsdk::core::fs_read(&path) {
            Ok(data) => {
                if path.ends_with(".jpg") || path.ends_with(".png") {
                    Message::PreviewLoaded(Ok(Handle::from_bytes(data)))
                } else if path.ends_with(".tif") || path.ends_with(".tiff") {
                    match utils::decode_tiff(&data) {
                        Ok((w, h, rgba)) => Message::PreviewLoaded(Ok(Handle::from_rgba(w, h, rgba))),
                        Err(e) => Message::PreviewLoaded(Err(format!("Failed to decode TIFF: {}", e))),
                    }
                } else { Message::PreviewLoaded(Err("Unsupported file format for preview".into())) }
            }
            Err(e) => Message::PreviewLoaded(Err(format!("Failed to read file: {}", e))),
        }
    })
}

pub fn handle_preview_loaded(state: &mut LocalState, res: Result<Handle, String>) -> Command<Message> {
    match res {
        Ok(handle) => {
            state.current_image = Some(handle);
            state.status_message = "Preview loaded".into();
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_clear_error(_state: &mut LocalState) -> Command<Message> { Command::none() }
pub fn handle_local_search(state: &mut LocalState, q: String) -> Command<Message> { state.downloaded_state.search_query = q; Command::none() }
pub fn handle_local_filter(state: &mut LocalState, f: crate::downloaded::FileFilter) -> Command<Message> { state.downloaded_state.filter = f; Command::none() }
pub fn handle_close_preview(state: &mut LocalState) -> Command<Message> { state.current_image = None; Command::none() }

// Helper functions

fn perform_browse_cmd(path: String) -> Command<Message> {
    Command::perform(async move {
        veldsdk::yield_now().await;
        let req = ListPathRequest { path: path.clone(), token: String::new() };
        match veldsdk::rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
            Ok(res_bytes) => {
                match ListPathResponse::decode(&res_bytes[..]) {
                    Ok(response) => Message::BrowsePathLoaded(Ok((path, response))),
                    Err(e) => Message::BrowsePathLoaded(Err(format!("Decode Error: {}", e))),
                }
            }
            Err(e) => Message::BrowsePathLoaded(Err(format!("Browse Error: {}", e))),
        }
    })
}

fn refresh_local_files(state: &mut LocalState) {
    let path = "data/dem/source";
    if let Ok(entries) = veldsdk::core::fs_list(path) {
        state.local_files = entries.into_iter().map(|name| {
            BrowserItem { s3_key: format!("{}/{}", path, name), name, is_folder: false, exists_locally: true }
        }).collect();
    }
}
