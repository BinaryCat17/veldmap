use std::sync::{Arc, Mutex};
use veldmap_api::data_browser::DownloadPressed;
use veldmap_api::dataprovider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded};
use veldsdk::core::Command;

use crate::state::{State, downloaded::DownloadStatus};
use crate::components::task_manager::TaskKind;

/// Пользователь нажал кнопку скачать
/// Просто публикуем запрос к data-provider и забываем
pub fn on_download_pressed(
    _state: Arc<Mutex<State>>,
    request: DownloadPressed,
) {
    let filename = request.s3_key.split('/').last().unwrap_or("file").to_string();
    
    veldsdk::publish!("data-provider/download", DownloadRequest {
        identifier: request.s3_key,
        destination: format!("data/dem/source/{}", filename),
    });
    // Fire-and-forget! Не ждём ответа.
}

/// Data-provider сообщил что загрузка началась
pub fn on_download_started(
    state: Arc<Mutex<State>>,
    event: DownloadStarted,
) {
    let filename = event.identifier.split('/').last().unwrap_or("file").to_string();
    
    let mut guard = state.lock().unwrap();
    
    // Добавляем в TaskManager
    guard.global.task_manager.spawn(
        event.task_id.clone(),
        TaskKind::Download { 
            task_id: event.task_id.clone(),
            s3_key: event.identifier.clone(), 
            filename: filename.clone(),
        }
    );
    
    // Добавляем в active_downloads
    guard.downloaded.active_downloads.insert(event.identifier.clone(), crate::state::downloaded::DownloadProgress {
        s3_key: event.identifier,
        task_id: event.task_id,
        progress: 0.0,
        status: DownloadStatus::Downloading,
    });
    
    guard.global.status_message = format!("Starting download: {}", filename);
}

/// Data-provider сообщил прогресс
pub fn on_download_progress(
    state: Arc<Mutex<State>>,
    event: DownloadProgress,
) {
    let mut guard = state.lock().unwrap();
    
    // Обновляем TaskManager
    guard.global.task_manager.update_progress(&event.task_id, event.progress);
    
    // Обновляем active_downloads
    for dl in guard.downloaded.active_downloads.values_mut() {
        if dl.task_id == event.task_id {
            dl.progress = event.progress;
            break;
        }
    }
}

/// Data-provider сообщил что загрузка завершена
pub fn on_downloaded(
    state: Arc<Mutex<State>>,
    event: Downloaded,
) {
    let mut guard = state.lock().unwrap();
    
    // Находим по task_id
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
}
