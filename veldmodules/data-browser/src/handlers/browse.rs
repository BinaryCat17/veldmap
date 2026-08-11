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
        // Содержимое накапливается ответами (см. on_list_path_result), поэтому
        // новый обход начинается с пустого списка, а не заменяет старый в конце:
        // иначе на экране до первого ответа стояла бы чужая папка.
        browse.items.clear();
        browse.listing.page = 0;
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
///
/// Из вида, у которого каталога нет (скачанное, поиск), переход открывает
/// каталог новой вкладкой: показать папку — это перейти в неё, а другого места
/// для этого в приложении нет.
pub fn on_enter(state: &mut State, path: String) {
    let Some((view, browse)) = state.active_browse_mut() else {
        if !path.is_empty() {
            super::nav::on_new_browse(state);
            if let Some((view, _)) = state.active_browse_mut() {
                request_path(state, view, path);
            }
        }
        return;
    };
    let target = if path.is_empty() { browse.current_path.clone() } else { path };
    request_path(state, view, target);
}

pub fn on_up(state: &mut State) {
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

/// Сколько записей забирать из одной папки. Хранилище отдаёт листинг
/// страницами, и страницы дочитываются одна за другой — иначе папка с сотней
/// продуктов показывала бы первые двести и молчала об остальных. Потолок нужен
/// затем, что корень бакета не кончается.
const MAX_ITEMS: usize = 1000;

/// Broadcast-топик: чей это ответ, знает таблица маршрутов. Не найден — не наш;
/// вид не найден — вкладку закрыли, пока ответ шёл, и показывать его негде.
/// Свой, но устаревший (пользователь успел уйти в другую папку) отбрасываем
/// тоже: его содержимое под нынешним путём было бы неправдой.
///
/// Ответ бывает не последним: пока хранилище отдаёт продолжение, запрос
/// остаётся тем же — та же корреляция, тот же учёт, — и с учёта снимается
/// только на последней странице.
pub fn on_list_path_result(
    state: &mut State,
    response: crate::proto::data_provider::ListPathResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.listings.take(&correlation_id) else { return };
    let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };

    if browse.request.status(&correlation_id) != veldsdk::Reply::Current {
        browse.request.settle(&correlation_id);
        return;
    }

    if !response.error.is_empty() {
        browse.request.settle(&correlation_id);
        browse.error = Some(response.error);
        browse.items.clear();
        return;
    }
    browse.error = None;

    browse.items.extend(response.entries.into_iter().map(|entry| {
        crate::module::state::browse::BrowseItem {
            is_folder: entry.key.ends_with('/'),
            name: entry.key.split('/').filter(|part| !part.is_empty()).next_back().unwrap_or("").to_string(),
            identifier: entry.key,
            size: entry.size,
            modified: entry.modified,
        }
    }));

    let more = !response.next_token.is_empty() && browse.items.len() < MAX_ITEMS;
    if !more {
        browse.request.settle(&correlation_id);
        return;
    }

    let path = browse.current_path.clone();
    state.listings.insert(correlation_id.clone(), view);
    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path,
        token: response.next_token,
    }, &correlation_id);
}
