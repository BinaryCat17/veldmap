use crate::{LocalState, AppMessage as Message};
use crate::common::ViewMode;
use crate::common::BrowserItem;
use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, ListPathRequest, ListPathResponse, DownloadRequest, DownloadResponse, SearchFilter, DataProduct};
use veldsdk::core::Command;
use veld_ui::core::*;
use crate::LocalConfig;

pub fn module_init(_cfg: LocalConfig) -> anyhow::Result<(LocalState, ())> {
    let state = LocalState {
        view_mode: ViewMode::Search,
        status_message: "VeldMap Data Browser Ready".to_string(),
        error_message: None,
        search_state: crate::search::SearchState::default(),
        search_results: Vec::new(),
        download_progress: None,
        active_download_task: None,
        active_image_task: None,
        current_image: None,
        current_gpu_image: None,
        downloaded_state: crate::downloaded::DownloadedState::default(),
        token_stack: Vec::new(),
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
        let res_bytes = veldsdk::rpc::host::call_service("data-provider", "search", veldsdk::prost::Message::encode_to_vec(&req)).map_err(|e| e.to_string())?;
        <SearchResponse as veldsdk::prost::Message>::decode(&res_bytes[..]).map_err(|e| e.to_string())
    }, Message::SearchResult)
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
    let req = ListPathRequest { path: prod.path.clone(), token: String::new() };
    Command::perform(async move {
        let res_bytes = veldsdk::rpc::host::call_service("data-provider", "list_path", veldsdk::prost::Message::encode_to_vec(&req)).map_err(|e| e.to_string())?;
        <ListPathResponse as veldsdk::prost::Message>::decode(&res_bytes[..]).map_err(|e| e.to_string())
    }, Message::ProductFilesLoaded)
}

pub fn handle_product_files_loaded(state: &mut LocalState, res: Result<ListPathResponse, String>) -> Command<Message> {
    match res {
        Ok(response) => {
            let local_files = veldsdk::core::raw::fs_list(&FsListRequest { path: "data/dem/source".into() }).map(|r| r.entries).unwrap_or_default();
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
            let local_files = veldsdk::core::raw::fs_list(&FsListRequest { path: "data/dem/source".into() }).map(|r| r.entries).unwrap_or_default();
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
    state.status_message = format!("Requesting {}...", filename);
    let dest = format!("data/dem/source/{}", filename);
    let req = DownloadRequest { identifier: s3_key, destination: dest };
    
    Command::perform(async move {
        let res_bytes = veldsdk::rpc::host::call_service("data-provider", "download", veldsdk::prost::Message::encode_to_vec(&req)).map_err(|e| e.to_string())?;
        let resp = <DownloadResponse as veldsdk::prost::Message>::decode(&res_bytes[..]).map_err(|e| e.to_string())?;
        if resp.success { Ok(resp.task_id) } else { Err(resp.error) }
    }, Message::DownloadStarted)
}

pub fn handle_download_started(state: &mut LocalState, res: Result<String, String>) -> Command<Message> {
    match res {
        Ok(task_id) => {
            state.active_download_task = Some(task_id);
            state.download_progress = Some(0.0);
            state.status_message = "Download started".into();
            return poll_progress();
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_update_progress(state: &mut LocalState) -> Command<Message> {
    if let Some(task_id) = &state.active_download_task {
        match veldsdk::core::raw::task_status(&TaskStatusRequest { task_id: task_id.clone() }) {
            Ok(status) => {
                state.download_progress = Some(status.progress);
                if status.completed {
                    let err = status.error.clone();
                    state.active_download_task = None;
                    state.download_progress = None;
                    
                    if err.is_empty() {
                        return on_download_finished(state, Ok(()));
                    } else {
                        return on_download_finished(state, Err(err));
                    }
                } else {
                    return poll_progress();
                }
            }
            Err(e) => { 
                state.error_message = Some(format!("Status check failed: {}", e));
                state.active_download_task = None;
                state.download_progress = None;
            }
        }
    }
    Command::none()
}

fn poll_progress() -> Command<Message> {
    Command::perform(async {
        veldsdk::yield_now().await;
    }, |_| Message::UpdateDownloadProgress)
}

pub fn handle_cancel_download(state: &mut LocalState) -> Command<Message> {
    if let Some(task_id) = &state.active_download_task {
        let _ = veldsdk::core::raw::task_cancel(&TaskCancelRequest { task_id: task_id.clone() });
        state.active_download_task = None;
        state.download_progress = None;
        state.status_message = "Download cancelled".into();
    }
    Command::none()
}

fn on_download_finished(state: &mut LocalState, res: Result<(), String>) -> Command<Message> {
    match res {
        Ok(_) => {
            state.status_message = "Download complete".into();
            if let ViewMode::Browse = state.view_mode {
                return perform_browse_cmd(state.current_browse_path.clone());
            } else if state.selected_product.is_some() {
                let local_files = veldsdk::core::raw::fs_list(&FsListRequest { path: "data/dem/source".into() }).map(|r| r.entries).unwrap_or_default();
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
    match veldsdk::core::raw::fs_delete(&FsDeleteRequest { path: path.clone() }) {
        Ok(_) => { state.status_message = format!("Deleted {}", path); refresh_local_files(state); }
        Err(e) => { state.error_message = Some(format!("Failed to delete {}: {}", path, e)); }
    }
    Command::none()
}

pub fn handle_view(state: &mut LocalState, path: String) -> Command<Message> {
    state.status_message = format!("Loading preview for {}...", path);
    
    match veldsdk::core::raw::image_info(&ImageInfoRequest { path: path.clone() }) {
        Ok(info) => {
            if !info.error.is_empty() {
                state.error_message = Some(format!("Image info error: {}", info.error));
                return Command::none();
            }
            
            match veldsdk::core::raw::image_load(&ImageLoadRequest { 
                path: path.clone(), target_width: 2048, target_height: 2048, preserve_aspect: true 
            }) {
                Ok(res) => {
                    if let Some(task) = res.task {
                        state.active_image_task = Some(task.task_id);
                        state.download_progress = Some(0.0);
                        return poll_image_progress();
                    }
                }
                Err(e) => {
                    state.error_message = Some(format!("Image load failed: {}", e));
                }
            }
        }
        Err(e) => {
            state.error_message = Some(format!("Failed to get image info: {}", e));
        }
    }
    Command::none()
}

pub fn handle_image_status(state: &mut LocalState) -> Command<Message> {
    if let Some(task_id) = &state.active_image_task {
        match veldsdk::core::raw::task_status(&TaskStatusRequest { task_id: task_id.clone() }) {
            Ok(status) => {
                state.download_progress = Some(status.progress);
                if status.completed {
                    let err = status.error.clone();
                    let handle = status.result_handle;
                    state.active_image_task = None;
                    state.download_progress = None;
                    
                    if !err.is_empty() {
                        state.error_message = Some(format!("Image load error: {}", err));
                    } else if let Some(h) = handle {
                        state.current_gpu_image = Some(h.clone());
                        state.status_message = "Image loaded to GPU".into();
                    }
                } else {
                    return poll_image_progress();
                }
            }
            Err(e) => { 
                state.error_message = Some(format!("Status check failed: {}", e));
                state.active_image_task = None;
                state.download_progress = None;
            }
        }
    }
    Command::none()
}

fn poll_image_progress() -> Command<Message> {
    Command::perform(async {
        veldsdk::yield_now().await;
    }, |_| Message::UpdateDownloadProgress)
}

pub fn handle_preview_loaded(state: &mut LocalState, res: Result<u64, String>) -> Command<Message> {
    match res {
        Ok(handle) => {
            state.current_image = Some(handle);
            state.status_message = "Preview loaded".into();
        }
        Err(e) => { state.error_message = Some(e); }
    }
    Command::none()
}

pub fn handle_clear_error(state: &mut LocalState) -> Command<Message> { 
    state.error_message = None;
    Command::none() 
}
pub fn handle_local_search(state: &mut LocalState, q: String) -> Command<Message> { state.downloaded_state.search_query = q; Command::none() }
pub fn handle_local_filter(state: &mut LocalState, f: crate::downloaded::FileFilter) -> Command<Message> { state.downloaded_state.filter = f; Command::none() }
pub fn handle_close_preview(state: &mut LocalState) -> Command<Message> { state.current_image = None; Command::none() }

// Helper functions

fn perform_browse_cmd(path: String) -> Command<Message> {
    let p = path.clone();
    let req = ListPathRequest { path: path.clone(), token: String::new() };
    Command::perform(async move {
        let res_bytes = veldsdk::rpc::host::call_service("data-provider", "list_path", veldsdk::prost::Message::encode_to_vec(&req)).map_err(|e| e.to_string())?;
        let resp = <ListPathResponse as veldsdk::prost::Message>::decode(&res_bytes[..]).map_err(|e| e.to_string())?;
        Ok((p, resp))
    }, Message::BrowsePathLoaded)
}

fn refresh_local_files(state: &mut LocalState) {
    let path = "data/dem/source";
    if let Ok(res) = veldsdk::core::raw::fs_list(&FsListRequest { path: path.into() }) {
        state.local_files = res.entries.into_iter().map(|name| {
            BrowserItem { s3_key: format!("{}/{}", path, name), name, is_folder: false, exists_locally: true }
        }).collect();
    }
}