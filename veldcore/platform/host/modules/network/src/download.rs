//! Потоковое скачивание файла (топик network/on_fs_download): прогресс и
//! результат доставляются событиями. Операция учтена платформой с самой
//! публикации запроса (топик объявлен `cancellable: true`), и снимется с
//! учёта терминальным `on_fs_download_result`; фасаду Tasks остаётся отдать
//! abort-хендл, чтобы её было чем убить.
//!
//! Докачка: пишем в `<destination>.part`, переименовываем в конечное имя
//! только по успешному завершению. Обрыв (ошибка, отмена — AbortHandle::abort()
//! просто дропает future, ничего после точки .await не выполняется) оставляет
//! .part на диске как есть — это осознанная пауза, а не мусор: следующий
//! on_fs_download с тем же destination увидит его размер и допросит сервер
//! Range-заголовком с этого места, а не начнёт с нуля. Явное удаление — на
//! пользователе (fs/on_delete), не на этом обработчике.

use super::State;
use veldmap_host_util::{Caller, HostContext};
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{
    FsDownloadRequest, FsDownloadResponse, FsDownloadProgress,
};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use std::fs;
use std::path::PathBuf;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

/// Суффикс недокачанного файла. По ту сторону провода его же читает
/// data-library (`storage::PART_SUFFIX`) как признак «начато, но не доведено»;
/// равенство держит таблица пар `buildgen/tests/test_wire_pairs.py`.
const PART_SUFFIX: &str = ".part";

/// Логирует провал скачивания и уведомляет подписчиков терминальным
/// on_fs_download_result — им же операция снимается с учёта.
fn fail_download(ctx: &HostContext, correlation_id: &str, error: String) -> String {
    log::warn!(target: "network", "Скачивание {} не удалось: {}", correlation_id, error);
    bus::emit::on_fs_download_result(&*ctx.publisher, &FsDownloadResponse {
        error: error.clone(),
    }, correlation_id);
    error
}

pub fn on_fs_download(state: &State, req: FsDownloadRequest, caller: Caller) {
    if !is_path_safe(&req.path) {
        bus::emit::on_fs_download_result(&*state.ctx.publisher, &FsDownloadResponse {
            error: format!("Путь за пределами дозволенного: {}", req.path),
        }, &caller.correlation);
        return;
    }

    let ctx = state.ctx.clone();
    let path = resolve_path(&ctx, &req.path);
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    log::info!(target: "network", "Запрошено скачивание: {}", req.path);
    // Суффикс, а не замена расширения (set_extension) — иначе "foo.tif" и
    // "foo.zip" в одной папке схлопнулись бы в один и тот же "foo.part".
    let part_path: PathBuf = {
        let mut s = path.clone().into_os_string();
        s.push(PART_SUFFIX);
        s.into()
    };

    // Операция уже учтена диспетчером: её топик объявлен `cancellable: true`,
    // владелец — паблишер запроса. Нам остаётся сделать её убиваемой, отдав
    // фасаду abort-хендл фьючерса.
    state.tasks.spawn(&caller, |correlation_id| async move {
        // Существующий .part с предыдущей попытки — точка возобновления:
        // узнаём его размер ДО запроса, чтобы попросить сервер прислать хвост.
        let resume_offset = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);

        // Докачка — это тот же Range-запрос, что и чтение окна, только хвост
        // тянется одним соединением потоком, а не блоками (см. http::get).
        let range = (resume_offset > 0).then_some((resume_offset, 0));
        let builder = super::http::get(&req.url, &req.headers, range);

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

        // Докачка стартует не с нуля: без этого события UI держит счётчик на
        // 0/0 до первого нового процента (или мегабайта) — то есть не
        // показывает byte-прогресс именно в момент, где докачка возобновилась,
        // а первым что-то покажет только когда новых байт наберётся на шаг
        // троттлинга ниже.
        if resuming {
            bus::emit::on_fs_download_progress(&*ctx.publisher, &FsDownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: total_size,
            }, &correlation_id);
        }

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
                                return Err(fail_download(&ctx, &correlation_id, format!("Ошибка записи: {}", e)));
                            }
                            downloaded += chunk.len() as u64;
                            // Прогресс событием, с троттлингом: по целым процентам,
                            // если известен total_size, иначе по мегабайтам.
                            if total_size > 0 {
                                let percent = (downloaded * 100 / total_size) as u32;
                                if percent > last_percent {
                                    last_percent = percent;
                                    bus::emit::on_fs_download_progress(&*ctx.publisher, &FsDownloadProgress {
                                        downloaded_bytes: downloaded,
                                        total_bytes: total_size,
                                    }, &correlation_id);
                                }
                            } else if downloaded - last_reported_bytes >= BYTES_THROTTLE {
                                last_reported_bytes = downloaded;
                                bus::emit::on_fs_download_progress(&*ctx.publisher, &FsDownloadProgress {
                                    downloaded_bytes: downloaded,
                                    total_bytes: 0,
                                }, &correlation_id);
                            }
                        }
                        Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("Обрыв потока: {}", e))),
                    }
                }
                let _ = async_file.flush().await;
            }
            Err(e) => return Err(fail_download(&ctx, &correlation_id, format!("Файл не открылся: {}", e))),
        }

        // Атомарно проявляем файл под конечным именем только теперь: до
        // этой строки на диске в любой прерванной ветке лежит только .part,
        // а не файл под именем, которое fs/list отдаёт как "скачано".
        if let Err(e) = tokio::fs::rename(&part_path, &path).await {
            return Err(fail_download(&ctx, &correlation_id, format!("Переименование не удалось: {}", e)));
        }

        log::info!(target: "network", "Скачивание {} завершено ({}/{} байт)", correlation_id, downloaded, total_size);
        bus::emit::on_fs_download_result(&*ctx.publisher, &FsDownloadResponse {
            error: String::new(),
        }, &correlation_id);
        Ok(())
    });
}
