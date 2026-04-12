use std::sync::{Arc, Mutex};
use veldmap_api::dataprovider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded};
use veld_ui::proto::UiEventResponse;

use crate::state::{State, downloaded::DownloadStatus};
use crate::components::task_manager::TaskKind;

/// Пользователь нажал кнопку скачать
pub fn on_download_pressed(
    state: Arc<Mutex<State>>,
    event: UiEventResponse,
) -> anyhow::Result<()> {
    let s3_key = event.value;
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    
    if !s3_key.is_empty() {
        veldsdk::publish!("data-provider/download", DownloadRequest {
            identifier: s3_key,
            destination: format!("data/dem/source/{}", filename),
        });
    }
    
    Ok(())
}

/// Пользователь нажал кнопку просмотра
pub fn on_view_pressed(
    state: Arc<Mutex<State>>,
    event: UiEventResponse,
) -> anyhow::Result<()> {
    let value = event.value;
    if value.is_empty() { return Ok(()); }
    
    let mut guard = state.lock().unwrap();
    guard.current_screen = crate::state::Screen::Preview;
    guard.preview.current_file = value.clone();
    guard.preview.is_loading = true;
    
    // Запрашиваем загрузку изображения у хоста
    veldsdk::publish!("image/load", veldsdk::rpc::core::ImageLoadRequest {
        path: value,
        target_width: 2048,
        target_height: 2048,
        preserve_aspect: true,
    });
    
    Ok(())
}

/// Data-provider сообщил что загрузка началась
pub fn on_download_started(
    state: Arc<Mutex<State>>,
    event: DownloadStarted,
) -> anyhow::Result<()> {
    let filename = event.identifier.split('/').last().unwrap_or("file").to_string();
    
    let mut guard = state.lock().unwrap();
    
    guard.global.task_manager.spawn(
        event.task_id.clone(),
        TaskKind::Download { 
            task_id: event.task_id.clone(),
            s3_key: event.identifier.clone(), 
            filename: filename.clone(),
        }
    );
    
    guard.downloaded.active_downloads.insert(event.identifier.clone(), crate::state::downloaded::DownloadProgress {
        s3_key: event.identifier,
        task_id: event.task_id,
        progress: 0.0,
        status: DownloadStatus::Downloading,
    });
    
    guard.global.status_message = format!("Starting download: {}", filename);
    crate::view::render(&mut guard);
    Ok(())
}

pub fn on_download_progress(
    state: Arc<Mutex<State>>,
    event: DownloadProgress,
) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.global.task_manager.update_progress(&event.task_id, event.progress);
    
    for dl in guard.downloaded.active_downloads.values_mut() {
        if dl.task_id == event.task_id {
            dl.progress = event.progress;
            break;
        }
    }
    crate::view::render(&mut guard);
    Ok(())
}

pub fn on_downloaded(
    state: Arc<Mutex<State>>,
    event: Downloaded,
) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    
    let s3_key = guard.downloaded.active_downloads
        .iter()
        .find(|(_, dl)| dl.task_id == event.task_id)
        .map(|(k, _)| k.clone());
    
    if let Some(key) = s3_key {
        let filename = key.split('/').last().unwrap_or("file").to_string();
        guard.downloaded.active_downloads.remove(&key);
        guard.global.task_manager.finish(&event.task_id);
        
        if event.success {
            guard.global.status_message = format!("Downloaded: {}", filename);
        } else {
            guard.global.error_message = Some(format!("Download failed: {}", event.error));
            guard.global.task_manager.fail(&event.task_id, event.error);
        }
    }
    crate::view::render(&mut guard);
    Ok(())
}
