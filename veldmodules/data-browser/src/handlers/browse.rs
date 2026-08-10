use crate::module::state::{State, ViewId, ViewKind};

/// Запрашивает листинг пути у названного вида и переводит его в loading.
/// Единственная точка входа — и для навигации в папку, и для «вверх», и для
/// открытия вкладки (см. handlers::nav): три копии этих десяти строк
/// расходились в мелочах.
pub fn request_path(state: &mut State, view: ViewId, path: String) {
    let correlation_id = {
        let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };
        browse.current_path = path.clone();
        browse.error = None;
        browse.request.begin()
    };
    state.listings.insert(correlation_id.clone(), view);

    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path,
        token: String::new(),
    }, &correlation_id);
}

/// Перейти в папку. Пустой путь — перечитать текущую: так же приходит и
/// «обновить», у которого своего пути нет.
pub fn on_browse(state: &mut State, path: String) {
    let Some((view, browse)) = state.active_browse_mut() else { return };
    let target = if path.is_empty() { browse.current_path.clone() } else { path };
    request_path(state, view, target);
}

pub fn on_browse_up(state: &mut State) {
    let Some((view, browse)) = state.active_browse_mut() else { return };
    let mut path = browse.current_path.clone();

    if path.ends_with('/') {
        path.pop();
    }
    match path.rfind('/') {
        Some(idx) => path.truncate(idx + 1),
        None => path.clear(), // Root
    }
    request_path(state, view, path);
}

/// Broadcast-топик: чей это ответ, знает таблица маршрутов. Не найден — не наш;
/// вид не найден — вкладку закрыли, пока ответ шёл, и показывать его негде.
/// Свой, но устаревший (пользователь успел уйти в другую папку) отбрасываем
/// тоже: его содержимое под нынешним путём было бы неправдой.
pub fn on_list_path_result(
    state: &mut State,
    response: crate::proto::data_provider::ListPathResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.listings.take(&correlation_id) else { return };
    let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };

    if browse.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        browse.error = Some(response.error);
        browse.items = Vec::new();
        return;
    }
    browse.error = None;

    browse.items = response.items.into_iter().map(|s| {
        let is_folder = s.ends_with('/');
        crate::module::state::browse::BrowseItem {
            identifier: s.clone(),
            name: s.split('/').filter(|x| !x.is_empty()).next_back().unwrap_or("").to_string(),
            is_folder,
        }
    }).collect();
}
