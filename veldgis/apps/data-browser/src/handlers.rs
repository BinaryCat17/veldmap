use crate::{LocalState, AppMessage as Message};
use crate::common::ViewMode;
use crate::common::BrowserItem;
use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, ListPathRequest, ListPathResponse, DownloadRequest, DownloadResponse, SearchFilter, DataProduct};
use veldmap_gis_api::raw as gis_rpc;
use veldsdk::core::Command;
use veld_ui::core::*;
use crate::LocalConfig;
use veldsdk::core::task::{TaskUpdate, TaskStatus};

pub fn module_init(_cfg: LocalConfig) -> anyhow::Result<(LocalState, ())> {
    let state = LocalState {
        view_mode: ViewMode::Search,
        status_message: "VeldMap Data Browser Ready".to_string(),
        error_message: None,
        search_state: crate::search::SearchState::default(),
        search_task: TaskStatus::Idle,
        browse_task: TaskStatus::Idle,
        download_task: TaskStatus::Idle,
        downloading_key: None,
        image_task: TaskStatus::Idle,
        search_results: Vec::new(),
        download_progress: None,
        current_image: None,
        current_gpu_image: None,
        downloaded_state: crate::downloaded::DownloadedState::default(),
        token_stack: Vec::new(),
        current_page_token: String::new(),
        next_token: None,
        current_browse_path: String::new(),
        selected_product: None,
        product_files: Vec::new(),
        browse_items: Vec::new(),
        local_files: Vec::new(),
    };
    Ok((state, ()))
}

pub fn handle_switch_mode(state: &mut LocalState, mode: ViewMode) -> Command<Message> {
    state.view_mode = mode;
    state.current_image = None;
    state.download_progress = None;
    state.selected_product = None;
    if state.view_mode == ViewMode::Browse && state.browse_items.is_empty() {
        return handle_browse_path(state, String::new());
    } else if state.view_mode == ViewMode::Downloaded {
        refresh_local_files(state);
    }
    Command::none()
}

pub fn handle_search_press(state: &mut LocalState) -> Command<Message> {
    let mut filters = Vec::new();
    match state.search_state.filter_type {
        crate::search::SearchFilterType::GridId => filters.push(SearchFilter { name: "gridId".into(), value: state.search_state.query.clone() }),
        crate::search::SearchFilterType::Collection => filters.push(SearchFilter { name: "Collection".into(), value: state.search_state.query.clone() }),
        _ => {}
    }
    let q = if state.search_state.filter_type == crate::search::SearchFilterType::General { state.search_state.query.clone() } else { String::new() };
    let req = SearchRequest { query: q, filters };

    gis_rpc::search_task(req, Message::SearchUpdate)
}

pub fn handle_search_update(state: &mut LocalState, update: TaskUpdate<SearchResponse>) -> Command<Message> {
    state.search_task.handle(update);
    if let TaskStatus::Finished(res) = &state.search_task {
        if !res.error.is_empty() {
            state.error_message = Some(format!("Search API Error: {}", res.error));
        } else {
            state.search_results = res.products.clone();
            state.status_message = format!("Found {} results", state.search_results.len());
        }
    } else if let Some(err) = state.search_task.error() {
        state.error_message = Some(format!("Search Task Failed: {}", err));
    }
    Command::none()
}

pub fn handle_browse_path(state: &mut LocalState, path: String) -> Command<Message> {
    state.status_message = format!("Listing /{}...", path);
    state.browse_items.clear();
    state.next_token = None;
    state.token_stack.clear();
    state.current_page_token = String::new();
    state.current_browse_path = path.clone();
    
    let req = ListPathRequest { path, token: String::new() };
    gis_rpc::list_path_task(req, Message::BrowseUpdate)
}

pub fn handle_browse_update(state: &mut LocalState, update: TaskUpdate<ListPathResponse>) -> Command<Message> {
    state.browse_task.handle(update);
    
    if let TaskStatus::Finished(response) = &state.browse_task {
        if !response.error.is_empty() {
            state.error_message = Some(format!("S3 Error: {}", response.error));
        } else {
            let local_files = veldsdk::core::raw::fs_list(&FsListRequest { path: "data/dem/source".into() }).map(|r| r.entries).unwrap_or_default();
            state.browse_items = response.items.iter().map(|s3_key| {
                let is_folder = s3_key.ends_with('/');
                let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(&s3_key).to_string();
                let exists_locally = !is_folder && local_files.contains(&name);
                BrowserItem { s3_key: s3_key.clone(), name, is_folder, exists_locally }
            }).collect();
            
            state.next_token = if response.next_token.is_empty() { None } else { Some(response.next_token.clone()) };
            state.status_message = format!("Loaded {} items", state.browse_items.len());
        }
    } else if let Some(err) = state.browse_task.error() {
        state.error_message = Some(format!("Browse Task Failed: {}", err));
    }
    Command::none()
}

pub fn handle_download(state: &mut LocalState, s3_key: String) -> Command<Message> {
    state.downloading_key = Some(s3_key.clone());
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    let dest = format!("data/dem/source/{}", filename);
    let req = DownloadRequest { identifier: s3_key, destination: dest };
    
    gis_rpc::download_task(req, Message::DownloadUpdate)
}

pub fn handle_download_update(state: &mut LocalState, update: TaskUpdate<DownloadResponse>) -> Command<Message> {
    state.download_task.handle(update);
    
    if let TaskStatus::Finished(res) = &state.download_task {
        state.downloading_key = None;
        if !res.error.is_empty() {
            state.error_message = Some(format!("Download Error: {}", res.error));
        } else {
            state.status_message = "Download complete".into();
            refresh_local_files(state);
        }
    } else if let Some(err) = state.download_task.error() {
        state.downloading_key = None;
        state.error_message = Some(format!("Download Task Failed: {}", err));
    }
    Command::none()
}

pub fn handle_view(_state: &mut LocalState, path: String) -> Command<Message> {
    let req = ImageLoadRequest { 
        path, target_width: 2048, target_height: 2048, preserve_aspect: true 
    };
    veldsdk::core::raw::image_load_task(req, Message::ImageUpdate)
}

pub fn handle_image_update(state: &mut LocalState, update: TaskUpdate<veldsdk::rpc::core::ResourceHandle>) -> Command<Message> {
    state.image_task.handle(update);
    if let TaskStatus::Finished(handle) = &state.image_task {
        state.current_gpu_image = Some(handle.clone());
    }
    Command::none()
}

// Stubs for other handlers
pub fn handle_search_input(state: &mut LocalState, q: String) -> Command<Message> { state.search_state.query = q; Command::none() }
pub fn handle_search_filter(state: &mut LocalState, ft: crate::search::SearchFilterType) -> Command<Message> { state.search_state.filter_type = ft; Command::none() }
pub fn handle_product_selected(state: &mut LocalState, prod: DataProduct) -> Command<Message> { state.selected_product = Some(prod.name); Command::none() }
pub fn handle_product_files_loaded(_state: &mut LocalState, _res: Result<ListPathResponse, String>) -> Command<Message> { Command::none() }
pub fn handle_back_to_list(state: &mut LocalState) -> Command<Message> { state.selected_product = None; Command::none() }
pub fn handle_next_page(state: &mut LocalState) -> Command<Message> {
    if let Some(token) = state.next_token.clone() {
        state.token_stack.push(state.current_page_token.clone());
        state.current_page_token = token.clone();
        
        let req = ListPathRequest { 
            path: state.current_browse_path.clone(), 
            token 
        };
        gis_rpc::list_path_task(req, Message::BrowseUpdate)
    } else {
        Command::none()
    }
}

pub fn handle_prev_page(state: &mut LocalState) -> Command<Message> {
    if let Some(token) = state.token_stack.pop() {
        state.current_page_token = token.clone();
        let req = ListPathRequest { 
            path: state.current_browse_path.clone(), 
            token 
        };
        gis_rpc::list_path_task(req, Message::BrowseUpdate)
    } else {
        Command::none()
    }
}

pub fn handle_browse_up(state: &mut LocalState) -> Command<Message> {
    let current = state.current_browse_path.trim_end_matches('/');
    if current.is_empty() {
        return Command::none();
    }
    
    let parent = if let Some(last_slash) = current.rfind('/') {
        format!("{}/", &current[..last_slash])
    } else {
        String::new()
    };

    handle_browse_path(state, parent)
}
pub fn handle_delete(state: &mut LocalState, path: String) -> Command<Message> { 
    let _ = veldsdk::core::raw::fs_delete(&FsDeleteRequest { path });
    refresh_local_files(state);
    Command::none()
}
pub fn handle_clear_error(state: &mut LocalState) -> Command<Message> { state.error_message = None; Command::none() }
pub fn handle_local_search(state: &mut LocalState, q: String) -> Command<Message> { state.downloaded_state.search_query = q; Command::none() }
pub fn handle_local_filter(state: &mut LocalState, f: crate::downloaded::FileFilter) -> Command<Message> { state.downloaded_state.filter = f; Command::none() }
pub fn handle_close_preview(state: &mut LocalState) -> Command<Message> { state.current_image = None; state.current_gpu_image = None; Command::none() }
pub fn handle_cancel_download(state: &mut LocalState) -> Command<Message> {
    if let TaskStatus::Running { task_id: Some(id), .. } = &mut state.download_task {
        let req = veldsdk::rpc::core::TaskCancelRequest { task_id: id.clone() };
        if let Ok(_) = veldsdk::core::raw::task_cancel(&req) {
            log::info!("Cancelled download task: {}", id);
        }
    }
    state.download_task = TaskStatus::Idle;
    state.downloading_key = None;
    state.status_message = "Download cancelled".into();
    Command::none()
}

fn refresh_local_files(state: &mut LocalState) {
    let path = "data/dem/source";
    if let Ok(res) = veldsdk::core::raw::fs_list(&FsListRequest { path: path.into() }) {
        state.local_files = res.entries.into_iter().map(|name| {
            BrowserItem { s3_key: format!("{}/{}", path, name), name, is_folder: false, exists_locally: true }
        }).collect();
    }
}
