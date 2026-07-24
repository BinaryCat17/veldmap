//! Потоковое скачивание файла (топик network/fs_download): прогресс и
//! результат доставляются событиями, жизненный цикл и отмена — через
//! фасад Tasks (см. module.rs): started/finished эмитит платформа.
//!
//! Докачка: пишем в `<destination>.part`, переименовываем в конечное имя
//! только по успешному завершению. Обрыв (ошибка, отмена — AbortHandle::abort()
//! просто дропает future, ничего после точки .await не выполняется) оставляет
//! .part на диске как есть — это осознанная пауза, а не мусор: следующий
//! on_fs_download с тем же destination увидит его размер и допросит сервер
//! Range-заголовком с этого места, а не начнёт с нуля. Явное удаление — на
//! пользователе (fs/on_delete), не на этом обработчике.

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
        // Существующий .part с предыдущей попытки — точка возобновления:
        // узнаём его размер ДО запроса, чтобы попросить сервер прислать хвост.
        let resume_offset = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);

        let client = reqwest::Client::new();
        let mut builder = client.get(&req.url);
        for (key, value) in req.headers { builder = builder.header(key, value); }
        if resume_offset > 0 {
            builder = builder.header("Range", format!("bytes={}-", resume_offset));
        }

        let res = match builder.send().await {
            Ok(r) => r,
            Err(e) => return Err(fail_download(&ctx, &correlation_id, e.to_string())),
        };

        if !res.status().is_success() {
            return Err(fail_download(&ctx, &correlation_id, format!("HTTP {}", res.status())));
        }

        // Сервер мог проигнорировать Range и прислать 200 с телом целиком
        // (не все раздачи это поддерживают) — тогда .part с предыдущей
        // попытки несовместим с тем, что сейчас летит по проводу, и
        // дописывать в него нельзя: начинаем файл заново.
        let resuming = resume_offset > 0 && res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut downloaded: u64 = if resuming { resume_offset } else { 0 };
        // Для 206 Content-Length — длина хвоста, а не всего файла.
        let total_size = if resuming {
            res.content_length().map(|remaining| remaining + resume_offset).unwrap_or(0)
        } else {
            res.content_length().unwrap_or(0)
        };
        let mut last_percent: u32 = if total_size > 0 { (downloaded * 100 / total_size) as u32 } else { 0 };
        // Троттлинг для ответов без Content-Length: процента нет, поэтому
        // считаем каждый мегабайт — иначе (total_size == 0) прогресс не
        // репортился бы вовсе, а UI не увидел бы даже байтовый счётчик.
        const BYTES_THROTTLE: u64 = 1 << 20;
        let mut last_reported_bytes: u64 = downloaded;
        let mut stream = res.bytes_stream();

        let file_result = if resuming {
            tokio::fs::OpenOptions::new().append(true).open(&part_path).await
        } else {
            tokio::fs::File::create(&part_path).await
        };

        match file_result {
            Ok(mut async_file) => {
                while let Some(chunk_res) = stream.next().await {
                    match chunk_res {
                        Ok(chunk) => {
                            if let Err(e) = async_file.write_all(&chunk).await {
                                return Err(fail_download(&ctx, &correlation_id, format!("Write error: {}", e)));
                            }
                            downloaded += chunk.len() as u64;
                            // Прогресс событием, с троттлингом: по целым процентам,
                            // если известен total_size, иначе по мегабайтам.
                            if total_size > 0 {
                                let percent = (downloaded * 100 / total_size) as u32;
                                if percent > last_percent {
                                    last_percent = percent;
                                    bus::emit::on_fs_download_progress(&*ctx.dispatcher, &FsDownloadProgress {
                                        correlation_id: correlation_id.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                        downloaded_bytes: downloaded,
                                        total_bytes: total_size,
                                    });
                                }
                            } else if downloaded - last_reported_bytes >= BYTES_THROTTLE {
                                last_reported_bytes = downloaded;
                                bus::emit::on_fs_download_progress(&*ctx.dispatcher, &FsDownloadProgress {
                                    correlation_id: correlation_id.clone(),
                                    progress: 0.0,
                                    downloaded_bytes: downloaded,
                                    total_bytes: 0,
                                });
                            }
                        }
                        Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("Stream error: {}", e))),
                    }
                }
                let _ = async_file.flush().await;
            }
            Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("File open error: {}", e))),
        }

        // Атомарно проявляем файл под конечным именем только теперь: до
        // этой строки на диске в любой прерванной ветке лежит только .part,
        // а не файл под именем, которое fs/list отдаёт как "скачано".
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
