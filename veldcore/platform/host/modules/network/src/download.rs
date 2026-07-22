//! Потоковое скачивание файла (топик network/fs_download): прогресс и
//! результат доставляются событиями, отмена — через реестр задач в State.

use super::State;
use veldmap_host_util::HostContext;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{
    FsDownloadRequest, FsDownloadResponse, FsDownloadProgress, TaskCancelRequest,
};
use veldmap_host_util::path::is_path_safe;
use std::fs;
use std::path::Path;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

/// Логирует провал скачивания и уведомляет подписчиков fs_download_result.
fn fail_download(ctx: &HostContext, correlation_id: &str, error: String) {
    log::warn!(target: "host", "Download {} failed: {}", correlation_id, error);
    bus::emit::fs_download_result(&*ctx.dispatcher, &FsDownloadResponse {
        error,
        correlation_id: correlation_id.to_string(),
    });
}

pub fn on_input_fs_download(state: &State, req: FsDownloadRequest, _requestor_id: u32) {
    if !is_path_safe(&req.path) {
        bus::emit::fs_download_result(&*state.ctx.dispatcher, &FsDownloadResponse {
            error: format!("Unsafe path: {}", req.path),
            correlation_id: req.correlation_id.clone(),
        });
        return;
    }

    // Единый идентификатор операции — correlation_id (генерируем, если не задан).
    let correlation_id = if req.correlation_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        req.correlation_id.clone()
    };
    let ctx = state.ctx.clone();
    if let Some(parent) = Path::new(&req.path).parent() { let _ = fs::create_dir_all(parent); }

    let cancel_key = correlation_id.clone();
    let join_handle = tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut builder = client.get(&req.url);
        for (key, value) in req.headers { builder = builder.header(key, value); }

        let res = match builder.send().await {
            Ok(r) => r,
            Err(e) => return fail_download(&ctx, &correlation_id, e.to_string()),
        };

        if !res.status().is_success() {
            return fail_download(&ctx, &correlation_id, format!("HTTP {}", res.status()));
        }

        let total_size = res.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut last_percent: u32 = 0;
        let mut stream = res.bytes_stream();

        match tokio::fs::File::create(&req.path).await {
            Ok(mut async_file) => {
                while let Some(chunk_res) = stream.next().await {
                    match chunk_res {
                        Ok(chunk) => {
                            if let Err(e) = async_file.write_all(&chunk).await {
                                return fail_download(&ctx, &correlation_id, format!("Write error: {}", e));
                            }
                            downloaded += chunk.len() as u64;
                            // Прогресс событием, с троттлингом по целым процентам.
                            if total_size > 0 {
                                let percent = (downloaded * 100 / total_size) as u32;
                                if percent > last_percent {
                                    last_percent = percent;
                                    bus::emit::fs_download_progress(&*ctx.dispatcher, &FsDownloadProgress {
                                        correlation_id: correlation_id.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                    });
                                }
                            }
                        }
                        Err(e) => return fail_download(&ctx, &correlation_id, format!("Stream error: {}", e)),
                    }
                }
                let _ = async_file.flush().await;
            }
            Err(e) => return fail_download(&ctx, &correlation_id, format!("File create error: {}", e)),
        }

        log::info!(target: "host", "Download {} completed ({}/{} bytes)", correlation_id, downloaded, total_size);
        bus::emit::fs_download_result(&*ctx.dispatcher, &FsDownloadResponse {
            error: String::new(),
            correlation_id: correlation_id.clone(),
        });
    });

    state.local_tasks.lock().unwrap().insert(cancel_key, join_handle.abort_handle());
}

/// Событие `network/cancel_download`: отмена фоновой задачи по correlation_id.
pub fn on_input_cancel_download(state: &State, req: TaskCancelRequest, _requestor_id: u32) {
    if let Some(handle) = state.local_tasks.lock().unwrap().remove(&req.task_id) {
        log::info!(target: "host", "NetworkService aborting task {}", req.task_id);
        handle.abort();
    }
}
