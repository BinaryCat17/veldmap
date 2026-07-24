use crate::proto::ui_service::proto::UiEventResponse;
use crate::module::state::State;

/// Браузинг запрошен (через UI событие)
pub fn on_browse(
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
    }

    state.browse.is_loading = true;
    state.browse.error = None;

    // Публикуем запрос к data-provider
    let correlation_id = state.browse.pending.begin(());
    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path: target_path,
        token: String::new(),
        correlation_id,
    });
}

pub fn on_browse_up(
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
    state.browse.error = None;

    let correlation_id = state.browse.pending.begin(());
    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path,
        token: String::new(),
        correlation_id,
    });
}

/// Broadcast-топик — сверяем correlation_id, чтобы не принять устаревший
/// или чужой ответ (например, от предыдущего on_browse_up).
pub fn on_list_path_result(
    state: &mut State,
    response: crate::proto::data_provider::ListPathResponse,
) {
    if state.browse.pending.take(&response.correlation_id).is_none() {
        return;
    }
    state.browse.is_loading = false;

    if !response.error.is_empty() {
        state.browse.error = Some(response.error);
        state.browse.items = Vec::new();
        return;
    }
    state.browse.error = None;

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
