use crate::state::{State, Screen, downloaded::LocalFile};
use veldsdk::rpc::core::FsListRequest;
use veld_ui::proto::UiEventResponse;

pub fn on_nav_browse(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Browse;
}

pub fn on_nav_search(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Search;
}

pub fn on_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Downloaded;
    
    // Запрашиваем список файлов при переходе на экран
    veldsdk::publish!("fs/list", FsListRequest {
        path: "data/dem/source".to_string(),
    });
}

/// Обработчик результата сканирования ФС
pub fn on_fs_list_result(state: &mut State, response: veldsdk::rpc::core::FsListResponse) {
    state.downloaded.local_files = response.entries.into_iter().map(|name| {
        LocalFile {
            path: format!("data/dem/source/{}", name),
            name,
            size: 0,
        }
    }).collect();
    // Рендер происходит автоматически в on_frame
}
