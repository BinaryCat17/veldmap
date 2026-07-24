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
use std::path::PathBuf;
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

/// Владеет .part-файлом скачивания, пока оно не завершится. Drop удаляет
/// файл, если downloading не закончилась явным `commit()` — так подчищается
/// обрывок и при ошибке, и при отмене: `AbortHandle::abort()` (см.
/// core::tasks::TaskRegistry::cancel) дропает future на месте, код после
/// точки `.await` не выполняется вообще, и Drop живых локальных — единственный
/// код, который платформа гарантированно исполнит. Тот же паттерн, что у
/// `OwnedResource` в veldsdk, только с коммитом вместо безусловного free.
struct PartFileGuard(Option<PathBuf>);

impl PartFileGuard {
    fn new(part_path: PathBuf) -> Self {
        Self(Some(part_path))
    }

    /// Успех: снимает файл с учёта — Drop его больше не тронет, вызывающий
    /// сам переименовывает .part в конечное имя.
    fn commit(mut self) -> PathBuf {
        self.0.take().expect("commit called twice")
    }
}

impl Drop for PartFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            // Sync: Drop не умеет .await, а remove_file — лёгкая unlink-операция.
            let _ = std::fs::remove_file(path);
        }
    }
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
    // Суффикс, а не замена расширения (set_extension) — иначе "foo.tif" и
    // "foo.zip" в одной папке схлопнулись бы в один и тот же "foo.part".
    let part_path: PathBuf = {
        let mut s = path.clone().into_os_string();
        s.push(".part");
        s.into()
    };

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

        let guard = PartFileGuard::new(part_path.clone());
        match tokio::fs::File::create(&part_path).await {
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

        // Атомарно проявляем файл под конечным именем только теперь: до
        // этой строки на диске в любой ветке (успех кода ниже ещё не
        // выполнился, ошибка, отмена) лежит только .part, а не файл под
        // именем, которое fs/list отдаёт как "скачано".
        let part_path = guard.commit();
        if let Err(e) = tokio::fs::rename(&part_path, &path).await {
            return Err(fail_download(&ctx, &correlation_id, format!("Rename error: {}", e)));
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
