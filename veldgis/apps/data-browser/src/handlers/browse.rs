use veld_ui::proto::UiEventResponse;
use crate::module::state::State;

/// Браузинг запрошен (через UI событие)
pub fn on_input_browse(
    state: &mut State,
    event: UiEventResponse,
) {
    let value = event.value;
    
    // Путь берется из value, если есть (нажатие на папку)
    let target_path = if !value.is_empty() {
        value
    } else {
        state.browse.current_path.clone()
    };
    
    if target_path != state.browse.current_path {
        state.browse.current_path = target_path.clone();
        state.browse.is_loading = true;
    }
    
    // Публикуем запрос к data-provider
    veldsdk::call!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path: target_path,
        token: String::new(),
    });
}

pub fn on_input_browse_up(
    state: &mut State,
    _event: UiEventResponse,
) {
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
    
    veldsdk::call!("data-provider/list_path", veldmap_api::dataprovider::ListPathRequest {
        path,
        token: String::new(),
    });
}

pub fn on_sub_list_path_result(
    state: &mut State,
    response: veldmap_api::dataprovider::ListPathResponse,
) {
    state.browse.is_loading = false;
    state.browse.items = response.items.into_iter().map(|s| {
        let is_folder = s.ends_with('/');
        crate::module::state::browse::BrowseItem {
            s3_key: s.clone(),
            name: s.split('/').filter(|x| !x.is_empty()).last().unwrap_or("").to_string(),
            is_folder,
        }
    }).collect();
    // Рендер происходит автоматически в on_frame
}
