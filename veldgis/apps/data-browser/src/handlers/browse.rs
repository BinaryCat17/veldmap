use std::sync::{Arc, Mutex};
use veldmap_api::ui::UiEventResponse;
use veldsdk::core::Command;

use crate::state::State;

/// Браузинг запрошен (через UI событие)
pub fn on_browse(
    state: Arc<Mutex<State>>,
    request: UiEventResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    
    // Путь берется из value, если есть (нажатие на папку)
    let target_path = if !request.value.is_empty() {
        request.value.clone()
    } else {
        guard.browse.current_path.clone() // Либо текущий путь (например, обновление)
    };
    
    if target_path != guard.browse.current_path {
        guard.browse.current_path = target_path.clone();
        guard.browse.is_loading = true;
    }
    
    // Публикуем запрос к data-provider
    veldsdk::publish!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path: target_path,
        token: String::new(),
    });
    
    crate::view::render(&guard);
    Command::none()
}

pub fn on_browse_up(
    state: Arc<Mutex<State>>,
    _request: UiEventResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    let mut path = guard.browse.current_path.clone();
    
    if path.ends_with('/') {
        path.pop();
    }
    if let Some(idx) = path.rfind('/') {
        path.truncate(idx + 1);
    } else {
        path = String::new(); // Root
    }
    
    guard.browse.current_path = path.clone();
    guard.browse.is_loading = true;
    
    veldsdk::publish!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path,
        token: String::new(),
    });
    
    crate::view::render(&guard);
    Command::none()
}

pub fn on_list_result(
    state: Arc<Mutex<State>>,
    response: veldmap_api::dataprovider::ListPathResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.browse.is_loading = false;
    guard.browse.items = response.items.into_iter().map(|s| {
        let is_folder = s.ends_with('/');
        crate::state::browse::BrowseItem {
            s3_key: s.clone(),
            name: s.split('/').filter(|x| !x.is_empty()).last().unwrap_or("").to_string(),
            is_folder,
        }
    }).collect();
    crate::view::render(&guard);
    Command::none()
}
