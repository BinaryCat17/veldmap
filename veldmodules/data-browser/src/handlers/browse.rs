use crate::proto::ui_service::proto::UiEventResponse;
use crate::module::state::State;

/// Запрашивает листинг пути у data-provider и переводит экран в loading.
/// Единственная точка входа — и для навигации в папку, и для "вверх", и для
/// захода на экран (см. handlers::nav): три копии этих десяти строк
/// расходились в мелочах.
pub fn request_path(state: &mut State, path: String) {
    state.browse.current_path = path.clone();
    state.browse.error = None;
    state.global.status_message = format!("Loading /{}...", path);

    let correlation_id = state.browse.request.begin();
    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path,
        token: String::new(),
    }, &correlation_id);
}

/// Браузинг запрошен (через UI событие). Путь берётся из value — это
/// нажатие на папку; пусто — перечитать текущий.
pub fn on_browse(state: &mut State, event: UiEventResponse) {
    let target = if event.value.is_empty() {
        state.browse.current_path.clone()
    } else {
        event.value
    };
    request_path(state, target);
}

pub fn on_browse_up(state: &mut State, _event: UiEventResponse) {
    let mut path = state.browse.current_path.clone();

    if path.ends_with('/') {
        path.pop();
    }
    match path.rfind('/') {
        Some(idx) => path.truncate(idx + 1),
        None => path.clear(), // Root
    }
    request_path(state, path);
}

/// Broadcast-топик — сверяем корреляцию. Отбрасываем не только чужой ответ,
/// но и свой устаревший: пока он шёл, пользователь мог уйти в другую папку,
/// и его содержимое под нынешним путём было бы неправдой.
pub fn on_list_path_result(
    state: &mut State,
    response: crate::proto::data_provider::ListPathResponse,
) {
    if state.browse.request.settle(&veldsdk::correlation()) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        state.browse.error = Some(response.error);
        state.browse.items = Vec::new();
        state.global.status_message = "Load failed".to_string();
        return;
    }
    state.browse.error = None;

    state.browse.items = response.items.into_iter().map(|s| {
        let is_folder = s.ends_with('/');
        crate::module::state::browse::BrowseItem {
            identifier: s.clone(),
            name: s.split('/').filter(|x| !x.is_empty()).last().unwrap_or("").to_string(),
            is_folder,
        }
    }).collect();
    state.global.status_message = format!("{} items", state.browse.items.len());
    // Рендер происходит автоматически в on_frame
}
