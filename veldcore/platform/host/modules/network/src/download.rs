//! Потоковое скачивание файла (топик network/fs_download): прогресс и
//! результат доставляются событиями, жизненный цикл и отмена — через
//! фасад Tasks (см. module.rs): started/finished эмитит платформа.

use super::State;
use veldmap_host_util::HostContext;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{
    FsDownloadRequest, FsDownloadResponse, FsDownloadProgress,
};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use std::fs;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

/// Логирует провал скачивания, уведомляет подписчиков fs_download_result
/// и возвращает текст ошибки — он же попадёт в tasks/task_finished.error.
fn fail_download(ctx: &HostContext, correlation_id: &str, error: String) -> String {
    log::warn!(target: "host", "Download {} failed: {}", correlation_id, error);
    bus::emit::on_fs_download_result(&*ctx.dispatcher, &FsDownloadResponse {
        error: error.clone(),
        correlation_id: correlation_id.to_string(),
    });
    error
}

pub fn on_fs_download(state: &State, req: FsDownloadRequest, requestor_id: u32) {
    if !is_path_safe(&req.path) {
        bus::emit::on_fs_download_result(&*state.ctx.dispatcher, &FsDownloadResponse {
            error: format!("Unsafe path: {}", req.path),
            correlation_id: req.correlation_id.clone(),
        });
        return;
    }

    let ctx = state.ctx.clone();
    let path = resolve_path(&ctx, &req.path);
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    let label = req.path.clone();

    // owner = инициатор запроса: отменить скачивание может он, хост
    // или сервис с его grant'ом (топик tasks/cancel).
    let spawned = state.tasks.spawn(&req.correlation_id, requestor_id, "fs_download", &label, |correlation_id| async move {
        let client = reqwest::Client::new();
        let mut builder = client.get(&req.url);
        for (key, value) in req.headers { builder = builder.header(key, value); }

        let res = match builder.send().await {
            Ok(r) => r,
            Err(e) => return Err(fail_download(&ctx, &correlation_id, e.to_string())),
        };

        if !res.status().is_success() {
            return Err(fail_download(&ctx, &correlation_id, format!("HTTP {}", res.status())));
        }

        let total_size = res.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut last_percent: u32 = 0;
        let mut stream = res.bytes_stream();

        match tokio::fs::File::create(&path).await {
            Ok(mut async_file) => {
                while let Some(chunk_res) = stream.next().await {
                    match chunk_res {
                        Ok(chunk) => {
                            if let Err(e) = async_file.write_all(&chunk).await {
                                return Err(fail_download(&ctx, &correlation_id, format!("Write error: {}", e)));
                            }
                            downloaded += chunk.len() as u64;
                            // Прогресс событием, с троттлингом по целым процентам.
                            if total_size > 0 {
                                let percent = (downloaded * 100 / total_size) as u32;
                                if percent > last_percent {
                                    last_percent = percent;
                                    bus::emit::on_fs_download_progress(&*ctx.dispatcher, &FsDownloadProgress {
                                        correlation_id: correlation_id.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                    });
                                }
                            }
                        }
                        Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("Stream error: {}", e))),
                    }
                }
                let _ = async_file.flush().await;
            }
            Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("File create error: {}", e))),
        }

        log::info!(target: "host", "Download {} completed ({}/{} bytes)", correlation_id, downloaded, total_size);
        bus::emit::on_fs_download_result(&*ctx.dispatcher, &FsDownloadResponse {
            error: String::new(),
            correlation_id: correlation_id.clone(),
        });
        Ok(())
    });

    if let Err(dup) = spawned {
        bus::emit::on_fs_download_result(&*state.ctx.dispatcher, &FsDownloadResponse {
            error: format!("Duplicate task id: {}", dup.0),
            correlation_id: dup.0,
        });
    }
}
