use crate::proto::data_provider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest};
use crate::proto::ui_service::proto::UiEventResponse;

use crate::module::state::{State, downloaded::{DownloadStatus, OriginSidecar, PROVIDER_NAME, filename_from_key}};
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

    // Origin — сразу, до ответа data-provider: даже если приложение упадёт
    // на первом байте, .part на диске уже будет знать, откуда он взялся.
    write_origin_sidecar(state, &filename, &s3_key);

    crate::calls::data_provider::on_download(&DownloadRequest {
        identifier: s3_key,
        destination: format!("data/dem/source/{}", filename),
    });
}

/// Пишет origin-sidecar (`<имя>.origin`) через fs/on_write — durable-копия
/// known_origins, переживающая рестарт приложения (см. handlers::nav —
/// восстановление читает её обратно для файлов без known_origins в памяти).
fn write_origin_sidecar(state: &mut State, filename: &str, identifier: &str) {
    state.downloaded.known_origins.insert(filename.to_string(), identifier.to_string());

    let sidecar = OriginSidecar { provider: PROVIDER_NAME.to_string(), identifier: identifier.to_string() };
    let Ok(json) = serde_json::to_vec(&sidecar) else { return };

    let Some(region_id) = veldsdk::abi::arena_alloc_cpu(json.len() as u64) else { return };
    veldsdk::abi::arena_write(region_id, 0, &json);
    if !veldsdk::abi::arena_grant_read(region_id, "fs") {
        veldsdk::abi::arena_free(region_id);
        return;
    }

    let correlation_id = state.downloaded.pending_sidecar_writes.begin(region_id);
    crate::calls::fs::on_write(&veldsdk::proto::fs::FsWriteRequest {
        path: format!("data/dem/source/{}.origin", filename),
        handle: Some(veldsdk::proto::core::ResourceHandle { id: region_id, size: json.len() as u64, content_hash: Vec::new() }),
        correlation_id,
    });
}

/// fs прочитал наш буфер (успешно или нет) — регион больше не нужен.
pub fn on_write_result(state: &mut State, response: veldsdk::proto::fs::FsWriteResult) {
    let Some(region_id) = state.downloaded.pending_sidecar_writes.take(&response.correlation_id) else { return; };
    veldsdk::abi::arena_free(region_id);
    if !response.error.is_empty() {
        veldsdk::vwarn!(veldsdk::FLAG_SDK, "[data-browser] failed to persist origin sidecar: {}", response.error);
    }
}

/// Ответ на fs/on_read origin-sidecar'а, запрошенный handlers::nav для файла
/// без known_origins в памяти (переживший рестарт .part или скачанный файл).
/// Отсутствие sidecar (файл скачан до появления этой фичи) — не ошибка,
/// просто re-download/докачка для него останутся недоступны.
pub fn on_read_result(state: &mut State, response: veldsdk::proto::fs::FsReadResult) {
    let Some(filename) = state.downloaded.pending_origin_reads.take(&response.correlation_id) else { return; };
    let Some(handle) = response.handle else { return; };

    let bytes = veldsdk::abi::arena_read(handle.id, 0, handle.size);
    veldsdk::abi::arena_free(handle.id);
    let Some(bytes) = bytes else { return; };

    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return; };
    if sidecar.provider != PROVIDER_NAME { return; }

    state.downloaded.known_origins.insert(filename.clone(), sidecar.identifier.clone());
    if let Some(f) = state.downloaded.local_files.iter_mut().find(|f| f.name == filename) {
        f.origin_key = Some(sidecar.identifier);
    }
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
            // known_origins для filename уже выставлен в write_origin_sidecar
            // при старте закачки — здесь просто финальный статус.
            state.global.status_message = format!("Downloaded: {}", filename);
        } else {
            state.global.error_message = Some(format!("Download failed: {}", event.error));
            state.global.task_manager.fail(&event.task_id, event.error);
        }
    }
    // Рендер происходит автоматически в on_frame
}

/// Пользователь нажал "удалить" на недокачанном файле (экран Downloaded).
/// Полностью скачанные файлы удалять пока негде в UI — эта кнопка только
/// для .part (см. browser_list рендер экрана Downloaded).
pub fn on_delete_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let path = event.value;
    if path.is_empty() { return; }

    // Sidecar живёт под "чистым" именем (без .part) — удаляем вместе с
    // файлом, чтобы не копить сироты; результат не отслеживаем, отсутствие
    // sidecar (например, файл скачан до этой фичи) — не ошибка.
    let base = path.strip_suffix(".part").unwrap_or(&path);
    crate::calls::fs::on_delete(&veldsdk::proto::fs::FsDeleteRequest {
        path: format!("{}.origin", base),
        correlation_id: String::new(),
    });

    let correlation_id = state.downloaded.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&veldsdk::proto::fs::FsDeleteRequest { path, correlation_id });
}

/// Broadcast-топик — сверяем correlation_id, чтобы не принять устаревший
/// или чужой ответ.
pub fn on_delete_result(
    state: &mut State,
    response: veldsdk::proto::fs::FsDeleteResult,
) {
    let Some(path) = state.downloaded.pending_delete.take(&response.correlation_id) else { return; };

    if !response.error.is_empty() {
        state.global.error_message = Some(format!("Failed to delete {}: {}", path, response.error));
        return;
    }
    state.downloaded.local_files.retain(|f| f.path != path);
}
