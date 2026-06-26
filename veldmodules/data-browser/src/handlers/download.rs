use crate::proto::dataprovider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded};
use crate::proto::ui::proto::UiEventResponse;

use crate::module::state::{State, downloaded::DownloadStatus};
use crate::module::components::task_manager::TaskKind;
use veldsdk::rpc::core::ImageLoadResult;

/// Пользователь нажал кнопку скачать
pub fn on_input_download_pressed(
    _state: &mut State,
    event: UiEventResponse,
) {
    let s3_key = event.value;
    let filename = s3_key.split('/').last().unwrap_or("file").to_string();
    
    if !s3_key.is_empty() {
        veldsdk::call!("data-provider/download", DownloadRequest {
            identifier: s3_key,
            destination: format!("data/dem/source/{}", filename),
        });
    }
}

/// Пользователь нажал кнопку просмотра
pub fn on_input_view_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let value = event.value;
    if value.is_empty() { return; }
    
    state.current_screen = crate::module::state::Screen::Preview;
    state.preview.current_path = value.clone();
    state.preview.is_loading = true;
    
    // Запрашиваем загрузку изображения у хоста
    veldsdk::output!("image/load", veldsdk::rpc::core::ImageLoadRequest {
        path: value,
        target_width: 2048,
        target_height: 2048,
        preserve_aspect: true,
        correlation_id: String::new(),
    });
}

/// Data-provider сообщил что загрузка началась
pub fn on_sub_download_started(
    state: &mut State,
    event: DownloadStarted,
) {
    let filename = event.identifier.split('/').last().unwrap_or("file").to_string();
    
    state.global.task_manager.spawn(
        event.task_id.clone(),
        TaskKind::Download { 
            task_id: event.task_id.clone(),
            s3_key: event.identifier.clone(), 
            filename: filename.clone(),
        }
    );
    
    state.downloaded.active_downloads.insert(event.identifier.clone(), crate::module::state::downloaded::DownloadProgress {
        s3_key: event.identifier,
        task_id: event.task_id,
        progress: 0.0,
        status: DownloadStatus::Downloading,
    });
    
    state.global.status_message = format!("Starting download: {}", filename);
    // Рендер происходит автоматически в on_frame
}

pub fn on_sub_download_progress(
    state: &mut State,
    event: DownloadProgress,
) {
    state.global.task_manager.update_progress(&event.task_id, event.progress);
    
    for dl in state.downloaded.active_downloads.values_mut() {
        if dl.task_id == event.task_id {
            dl.progress = event.progress;
            break;
        }
    }
    // Рендер происходит автоматически в on_frame
}

pub fn on_sub_downloaded(
    state: &mut State,
    event: Downloaded,
) {
    let s3_key = state.downloaded.active_downloads
        .iter()
        .find(|(_, dl)| dl.task_id == event.task_id)
        .map(|(k, _): (&String, &crate::module::state::downloaded::DownloadProgress)| k.clone());
    
    if let Some(key) = s3_key {
        let filename = key.split('/').last().unwrap_or("file").to_string();
        state.downloaded.active_downloads.remove(&key);
        state.global.task_manager.finish(&event.task_id);
        
        if event.success {
            state.global.status_message = format!("Downloaded: {}", filename);
        } else {
            state.global.error_message = Some(format!("Download failed: {}", event.error));
            state.global.task_manager.fail(&event.task_id, event.error);
        }
    }
    // Рендер происходит автоматически в on_frame
}

pub fn on_sub_load_result(
    state: &mut State,
    result: ImageLoadResult,
) {
    state.preview.is_loading = false;
    if result.error.is_empty() {
        if let Some(handle) = result.handle {
            state.preview.current_image = Some(handle.id);
        } else {
            state.preview.current_image = None;
            state.global.error_message = Some("Image loaded but handle is missing".to_string());
        }
    } else {
        state.preview.current_image = None;
        state.global.error_message = Some(format!("Image load failed: {}", result.error));
    }
}
