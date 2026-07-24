use crate::module::state::{State, Screen, downloaded::LocalFile};
use veldsdk::proto::fs::FsListRequest;
use crate::proto::ui_service::proto::UiEventResponse;

pub fn on_nav_browse(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Browse;

    // Запрашиваем листинг текущего пути при входе на экран
    state.browse.is_loading = true;
    state.browse.error = None;
    crate::calls::data_provider::on_list_path(&crate::proto::data_provider::ListPathRequest {
        path: state.browse.current_path.clone(),
        token: String::new(),
    });
}

pub fn on_nav_search(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Search;
}

pub fn on_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Downloaded;
    
    // Запрашиваем список файлов при переходе на экран
    let correlation_id = state.downloaded.pending_list.begin(());
    crate::calls::fs::on_list(&FsListRequest {
        path: "data/dem/source".to_string(),
        correlation_id,
    });
}

/// Обработчик результата сканирования ФС. Broadcast-топик — сверяем
/// correlation_id, чтобы не принять устаревший или чужой ответ.
pub fn on_list_result(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if state.downloaded.pending_list.take(&response.correlation_id).is_none() {
        return;
    }
    if !response.error.is_empty() {
        state.global.error_message = Some(format!("Failed to list files: {}", response.error));
        return;
    }
    state.downloaded.local_files = response.entries.into_iter().map(|name| {
        LocalFile {
            path: format!("data/dem/source/{}", name),
            name,
            size: 0,
        }
    }).collect();
    // Рендер происходит автоматически в on_frame
}
