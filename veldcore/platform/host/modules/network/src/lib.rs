#![recursion_limit = "256"]
//! Контракт сервиса — veldcore/proto/network.proto + network.schema.yaml;
//! типы и стабы топиков — из сгенерированных биндингов (platform/host/generated).

use veldmap_host_util::{AsyncNativeService, HostContext};
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{
    FsDownloadRequest, HttpTaskRequest, HttpTaskResponse,
    FsDownloadResponse, FsDownloadProgress, TaskCancelRequest
};
use veldmap_host_util::path::is_path_safe;
use veldmap_host_util::wire;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::Path;
use std::fs;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct NetworkService {
    ctx: Arc<HostContext>,
    /// AbortHandle'ы фоновых задач, ключ — correlation_id (id, известный инициатору),
    /// чтобы событие отмены могло адресовать задачу напрямую.
    local_tasks: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
}

/// Логирует провал скачивания и уведомляет подписчиков fs_download_result.
fn fail_download(ctx: &HostContext, correlation_id: &str, error: String) {
    log::warn!(target: "host", "Download {} failed: {}", correlation_id, error);
    bus::emit::fs_download_result(&*ctx.dispatcher, &FsDownloadResponse {
        error,
        correlation_id: correlation_id.to_string(),
    });
}

impl NetworkService {
    pub fn new(ctx: Arc<HostContext>) -> Self {
        Self { ctx, local_tasks: Arc::new(Mutex::new(HashMap::new())) }
    }

    async fn handle_fs_download(&self, payload: Vec<u8>) {
        let Some(req) = wire::decode_or_log::<FsDownloadRequest>(&payload, bus::topics::FS_DOWNLOAD) else { return };

        if !is_path_safe(&req.path) {
            bus::emit::fs_download_result(&*self.ctx.dispatcher, &FsDownloadResponse {
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
        let ctx = self.ctx.clone();
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

        self.local_tasks.lock().unwrap().insert(cancel_key, join_handle.abort_handle());
    }

    async fn handle_http(&self, payload: Vec<u8>) {
        let Some(req) = wire::decode_or_log::<HttpTaskRequest>(&payload, bus::topics::HTTP) else { return };

        let correlation_id = if req.correlation_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.correlation_id.clone()
        };
        let ctx = self.ctx.clone();
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

        self.local_tasks.lock().unwrap().insert(cancel_key, join_handle.abort_handle());
    }

    /// Событие `network/cancel_download`: отмена фоновой задачи по correlation_id.
    async fn handle_cancel_download(&self, payload: Vec<u8>) {
        let Some(req) = wire::decode_or_log::<TaskCancelRequest>(&payload, bus::topics::CANCEL_DOWNLOAD) else { return };
        if let Some(handle) = self.local_tasks.lock().unwrap().remove(&req.task_id) {
            log::info!(target: "host", "NetworkService aborting task {}", req.task_id);
            handle.abort();
        }
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for NetworkService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, _requestor_id: u32) {
        match topic {
            "fs_download" => self.handle_fs_download(payload).await,
            "http" => self.handle_http(payload).await,
            "cancel_download" => self.handle_cancel_download(payload).await,
            _ => log::warn!(target: "host", "Unknown network topic: {}", topic),
        }
    }
}

pub fn register_services(ctx: Arc<HostContext>) {
    let network_service = Arc::new(NetworkService::new(ctx.clone()));
    wire::subscribe_async(&ctx, &network_service, &[bus::topics::FS_DOWNLOAD, bus::topics::HTTP, bus::topics::CANCEL_DOWNLOAD]);
}
