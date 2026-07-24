use crate::proto::data_provider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest};
use crate::proto::ui_service::proto::UiEventResponse;

use crate::module::state::{State, downloaded::{DownloadStatus, filename_from_key}};
use crate::module::components::task_manager::TaskKind;

/// Пользователь нажал кнопку скачать.
/// Повторное нажатие на файл, который уже скачивается — отмена загрузки.
pub fn on_download_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let s3_key = event.value;
    let filename = filename_from_key(&s3_key);

    if s3_key.is_empty() { return; }

    // Отмена активной загрузки: data-provider пришлёт Downloaded{success:false},
    // и on_downloaded снимет задачу с панели.
    if let Some(dl) = state.downloaded.active_downloads.get(&s3_key) {
        let task_id = dl.task_id.clone();
        state.global.status_message = format!("Cancelling download: {}", filename);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
        return;
    }

    crate::calls::data_provider::on_download(&DownloadRequest {
        identifier: s3_key,
        destination: format!("data/dem/source/{}", filename),
    });
}

/// Пользователь нажал кнопку просмотра
pub fn on_view_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let value = event.value;
    if value.is_empty() { return; }

    state.current_screen = crate::module::state::Screen::Preview;
    state.preview.current_path = value;
    state.preview.is_loading = false;
    // TODO: wasm-модуль image ещё не реализован — загрузка превью появится вместе с ним.
    state.global.error_message = Some("Image preview: модуль image ещё не реализован".to_string());
}

/// Data-provider сообщил что загрузка началась
pub fn on_download_started(
    state: &mut State,
    event: DownloadStarted,
) {
    let filename = filename_from_key(&event.identifier);

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

pub fn on_download_progress(
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

pub fn on_downloaded(
    state: &mut State,
    event: Downloaded,
) {
    let s3_key = state.downloaded.active_downloads
        .iter()
        .find(|(_, dl)| dl.task_id == event.task_id)
        .map(|(k, _): (&String, &crate::module::state::downloaded::DownloadProgress)| k.clone());
    
    if let Some(key) = s3_key {
        let filename = filename_from_key(&key);
        state.downloaded.active_downloads.remove(&key);
        state.global.task_manager.finish(&event.task_id);

        if event.success {
            state.downloaded.known_origins.insert(filename.clone(), key);
            state.global.status_message = format!("Downloaded: {}", filename);
        } else {
            state.global.error_message = Some(format!("Download failed: {}", event.error));
            state.global.task_manager.fail(&event.task_id, event.error);
        }
    }
    // Рендер происходит автоматически в on_frame
}


