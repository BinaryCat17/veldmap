use std::sync::{Arc, Mutex};
use veldsdk::core::Command;

use crate::state::State;

/// Браузинг запрошен (через UI событие)
pub fn on_browse(
    state: &mut State,
    value: String,
) -> anyhow::Result<()> {
    // Путь берется из value, если есть (нажатие на папку)
    let target_path = if !value.is_empty() {
        value
    } else {
        state.browse.current_path.clone() // Либо текущий путь (например, обновление)
    };
    
    if target_path != state.browse.current_path {
        state.browse.current_path = target_path.clone();
        state.browse.is_loading = true;
    }
    
    // Публикуем запрос к data-provider
    veldsdk::publish!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path: target_path,
        token: String::new(),
    });
    
    Ok(())
}

pub fn on_browse_up(
    state: &mut State,
    _value: String,
) -> anyhow::Result<()> {
    let mut path = state.browse.current_path.clone();
    
    if path.ends_with('/') {
        path.pop();
    }
    if let Some(idx) = path.rfind('/') {
        path.truncate(idx + 1);
    } else {
        path = String::new(); // Root
    }
    
    state.browse.current_path = path.clone();
    state.browse.is_loading = true;
    
    veldsdk::publish!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path,
        token: String::new(),
    });
    
    Ok(())
}

pub fn on_list_result(
    state: Arc<Mutex<State>>,
    response: veldmap_api::dataprovider::ListPathResponse,
) -> anyhow::Result<()> {
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
    crate::view::render(&mut guard);
    Ok(())
}
