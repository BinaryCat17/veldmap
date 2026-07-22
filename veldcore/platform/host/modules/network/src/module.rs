//! Реализация сервиса network (контракт — veldcore/proto/network.schema.yaml).
//! Свободные обработчики on_input_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).

use veldmap_host_util::HostContext;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{
    FsDownloadRequest, HttpTaskRequest, HttpTaskResponse,
    FsDownloadResponse, FsDownloadProgress, TaskCancelRequest
};
use veldmap_host_util::path::is_path_safe;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::Path;
use std::fs;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct State {
    ctx: Arc<HostContext>,
    /// AbortHandle'ы фоновых задач, ключ — correlation_id (id, известный инициатору),
    /// чтобы событие отмены могло адресовать задачу напрямую.
    local_tasks: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

pub fn init(ctx: Arc<HostContext>) -> State {
    State { ctx, local_tasks: Mutex::new(HashMap::new()) }
}

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

pub fn on_input_http(state: &State, req: HttpTaskRequest, _requestor_id: u32) {
    let correlation_id = if req.correlation_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        req.correlation_id.clone()
    };
    let ctx = state.ctx.clone();
    let cancel_key = correlation_id.clone();

    log::info!(target: "host", "Received HTTP request: {} {} (correlation_id: {})", req.method, req.url, correlation_id);

    let join_handle = tokio::spawn(async move {
        log::info!(target: "host", "Executing HTTP request {}...", correlation_id);
        let client = reqwest::Client::new();
        let method = match req.method.to_uppercase().as_str() {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        };

        let mut builder = client.request(method, &req.url);
        for (k, v) in req.headers { builder = builder.header(k, v); }
        if !req.body.is_empty() { builder = builder.body(req.body); }

        let result = match builder.send().await {
            Ok(res) => {
                let status = res.status().as_u16() as u32;
                let body = res.bytes().await.unwrap_or_default().to_vec();
                Ok((status, body))
            }
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok((status, body)) => {
                log::info!(target: "host", "HTTP request {} finished with status {}", correlation_id, status);
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status, body, correlation_id: correlation_id.clone() });
            }
            Err(e) => {
                log::warn!(target: "host", "HTTP request {} failed: {}", correlation_id, e);
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status: 0, body: Vec::new(), correlation_id: correlation_id.clone() });
            }
        }
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
