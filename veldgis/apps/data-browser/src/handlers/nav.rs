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
    
    // Рендер теперь НЕ НУЖЕН внутри хэндлеров данных, так как SDK 
    // может вызвать render в конце handle_rpc, если мы так решим, 
    // но пока оставим явный рендер в on_ui_event.
    // Если это не-UI событие, то рендер нужно вызвать явно.
    let root = crate::view::build_root(state);
    let (w, h) = state.last_layout.as_ref().map(|l| (l.width, l.height)).unwrap_or((1024, 768));
    veld_ui::app::render("data-browser", root, &mut state.last_layout, w, h);
}
