use crate::module::state::{State, Screen, downloaded::LocalFile};
use veldsdk::rpc::core::FsListRequest;
use crate::proto::ui::proto::UiEventResponse;

pub fn on_input_nav_browse(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Browse;
}

pub fn on_input_nav_search(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Search;
}

pub fn on_input_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Downloaded;
    
    // Запрашиваем список файлов при переходе на экран
    crate::calls::fs::list(&FsListRequest {
        path: "data/dem/source".to_string(),
        correlation_id: veldsdk::generate_id(),
    });
}

/// Обработчик результата сканирования ФС
pub fn on_sub_list_result(state: &mut State, response: veldsdk::rpc::core::FsListResponse) {
    state.downloaded.local_files = response.entries.into_iter().map(|name| {
        LocalFile {
            path: format!("data/dem/source/{}", name),
            name,
            size: 0,
        }
    }).collect();
    // Рендер происходит автоматически в on_frame
}
