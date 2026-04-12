use std::sync::{Arc, Mutex};
use crate::state::{State, Screen, downloaded::LocalFile};
use veldsdk::rpc::core::FsListRequest;
use veld_ui::proto::UiEventResponse;

pub fn on_nav_browse(state: Arc<Mutex<State>>, _event: UiEventResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Browse;
    Ok(())
}

pub fn on_nav_search(state: Arc<Mutex<State>>, _event: UiEventResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Search;
    Ok(())
}

pub fn on_nav_downloaded(state: Arc<Mutex<State>>, _event: UiEventResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.current_screen = Screen::Downloaded;
    
    // Запрашиваем список файлов при переходе на экран
    veldsdk::publish!("fs/list", FsListRequest {
        path: "data/dem/source".to_string(),
    });
    
    Ok(())
}

/// Обработчик результата сканирования ФС
pub fn on_fs_list_result(state: Arc<Mutex<State>>, response: veldsdk::rpc::core::FsListResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.downloaded.local_files = response.entries.into_iter().map(|name| {
        LocalFile {
            path: format!("data/dem/source/{}", name),
            name,
            size: 0,
        }
    }).collect();
    
    // Принудительный рендер для не-UI события
    crate::view::render(&mut guard);
    Ok(())
}
