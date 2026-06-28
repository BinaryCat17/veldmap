#![recursion_limit = "256"]
use veldmap_host_core::dispatcher::{AsyncNativeService, Dispatcher};
use veldmap_host_core::core::{
    FsDownloadRequest, HttpTaskRequest, HttpTaskResponse,
    FsDownloadResponse, TaskResponse, TaskCreateRequest, TaskUpdateRequest
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::Path;
use std::fs;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use veldmap_host_core::setup::HostContext;

pub struct NetworkService {
    ctx: Arc<HostContext>,
    local_tasks: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
}

impl NetworkService {
    pub fn new(ctx: Arc<HostContext>) -> Self {
        Self { ctx, local_tasks: Arc::new(Mutex::new(HashMap::new())) }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }

    async fn handle_fs_download(&self, payload: Vec<u8>) {
        let req = match FsDownloadRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode FsDownloadRequest: {}", e);
                return;
            }
        };

        if !Self::is_path_safe(&req.path) {
            let correlation_id = req.correlation_id;
            let response = FsDownloadResponse {
                                error: String::new(),
                task: Some(TaskResponse { task_id: String::new() }),
                correlation_id: correlation_id.clone(),
            };
            self.ctx.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
            return;
        }

        let task_id = uuid::Uuid::new_v4().to_string();
        let correlation_id = if req.correlation_id.is_empty() {
            task_id.clone()
        } else {
            req.correlation_id.clone()
        };
        let ctx_clone = self.ctx.clone();
        if let Some(parent) = Path::new(&req.path).parent() { let _ = fs::create_dir_all(parent); }

        // Create task via RPC
        let _ = self.ctx.dispatcher.call("system", "task_create", TaskCreateRequest { task_id: task_id.clone() }.encode_to_vec(), 0);

        let task_id_inner = task_id.clone();
        let join_handle = tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut builder = client.get(&req.url);
            for (key, value) in req.headers { builder = builder.header(key, value); }
            
            let res = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                        task_id: task_id_inner.clone(),
                        progress: 0.0,
                        completed: true,
                        error: e.to_string(),
                        payload: Vec::new(),
                    }.encode_to_vec(), 0);
                    let response = FsDownloadResponse {
                                error: String::new(),
                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                        correlation_id: correlation_id.clone(),
                    };
                    ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
                    return;
                }
            };

            if !res.status().is_success() {
                let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                    task_id: task_id_inner.clone(),
                    progress: 0.0,
                    completed: true,
                    error: format!("HTTP {}", res.status()),
                    payload: Vec::new(),
                }.encode_to_vec(), 0);
                let response = FsDownloadResponse {
                                error: String::new(),
                    task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                    correlation_id: correlation_id.clone(),
                };
                ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
                return;
            }

            let total_size = res.content_length().unwrap_or(0);
            let mut downloaded: u64 = 0;
            let mut stream = res.bytes_stream();
            
            match tokio::fs::File::create(&req.path).await {
                Ok(mut async_file) => {
                    while let Some(chunk_res) = stream.next().await {
                        match chunk_res {
                            Ok(chunk) => {
                                if let Err(e) = async_file.write_all(&chunk).await {
                                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                                        task_id: task_id_inner.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                        completed: true,
                                        error: format!("Write error: {}", e),
                                        payload: Vec::new(),
                                    }.encode_to_vec(), 0);
                                    let response = FsDownloadResponse {
                                error: String::new(),
                                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                                        correlation_id: correlation_id.clone(),
                                    };
                                    ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
                                    return;
                                }
                                downloaded += chunk.len() as u64;
                                if total_size > 0 {
                                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                                        task_id: task_id_inner.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                        completed: false,
                                        error: String::new(),
                                        payload: Vec::new(),
                                    }.encode_to_vec(), 0);
                                }
                            }
                            Err(e) => {
                                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                                        task_id: task_id_inner.clone(),
                                        progress: downloaded as f32 / total_size as f32,
                                        completed: true,
                                        error: format!("Stream error: {}", e),
                                        payload: Vec::new(),
                                    }.encode_to_vec(), 0);
                                let response = FsDownloadResponse {
                                error: String::new(),
                                    task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                                    correlation_id: correlation_id.clone(),
                                };
                                ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
                                return;
                            }
                        }
                    }
                    let _ = async_file.flush().await;
                }
                Err(e) => {
                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                        task_id: task_id_inner.clone(),
                        progress: 0.0,
                        completed: true,
                        error: format!("File create error: {}", e),
                        payload: Vec::new(),
                    }.encode_to_vec(), 0);
                    let response = FsDownloadResponse {
                                error: String::new(),
                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                        correlation_id: correlation_id.clone(),
                    };
                    ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
                    return;
                }
            }

            let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                task_id: task_id_inner.clone(),
                progress: 1.0,
                completed: true,
                error: String::new(),
                payload: Vec::new(),
            }.encode_to_vec(), 0);
            let response = FsDownloadResponse {
                                error: String::new(),
                task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                correlation_id: correlation_id.clone(),
            };
            ctx_clone.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
        });

        self.local_tasks.lock().unwrap().insert(task_id, join_handle.abort_handle());
    }

    async fn handle_http(&self, payload: Vec<u8>) {
        let req = match HttpTaskRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode HttpTaskRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        let task_id = uuid::Uuid::new_v4().to_string();
        let ctx_clone = self.ctx.clone();
        let task_id_inner = task_id.clone();
        
        log::info!(target: "host", "Received HTTP request: {} {} (correlation_id: {})", req.method, req.url, correlation_id);

        let _ = self.ctx.dispatcher.call("system", "task_create", TaskCreateRequest { task_id: task_id.clone() }.encode_to_vec(), 0);

        let join_handle = tokio::spawn(async move {
            log::info!(target: "host", "Executing HTTP task {}...", task_id_inner);
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
                    log::info!(target: "host", "HTTP task {} finished with status {}", task_id_inner, status);
                    let response = HttpTaskResponse { status, body, correlation_id: correlation_id.clone() };
                    let payload = response.encode_to_vec();
                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                        task_id: task_id_inner.clone(),
                        progress: 1.0,
                        completed: true,
                        error: String::new(),
                        payload: payload.clone(),
                    }.encode_to_vec(), 0);
                    ctx_clone.dispatcher.publish("network/http_result", payload);
                }
                Err(e) => {
                    log::warn!(target: "host", "HTTP task {} failed: {}", task_id_inner, e);
                    let response = HttpTaskResponse { status: 0, body: Vec::new(), correlation_id: correlation_id.clone() };
                    let _ = ctx_clone.dispatcher.call("system", "task_update", TaskUpdateRequest {
                        task_id: task_id_inner.clone(),
                        progress: 0.0,
                        completed: true,
                        error: e.to_string(),
                        payload: Vec::new(),
                    }.encode_to_vec(), 0);
                    ctx_clone.dispatcher.publish("network/http_result", response.encode_to_vec());
                }
            }
        });

        self.local_tasks.lock().unwrap().insert(task_id, join_handle.abort_handle());
    }

    async fn handle_task_cancel_broadcast(&self, payload: Vec<u8>) {
        if let Ok(req) = veldmap_host_core::core::TaskCancelRequest::decode(&payload[..]) {
            if let Some(handle) = self.local_tasks.lock().unwrap().remove(&req.task_id) {
                log::info!(target: "host", "NetworkService aborting task {}", req.task_id);
                handle.abort();
            }
        }
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for NetworkService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, _requestor_id: u32) {
        match topic {
            "fs_download" => self.handle_fs_download(payload).await,
            "http" => self.handle_http(payload).await,
            "task_cancel_broadcast" => self.handle_task_cancel_broadcast(payload).await,
            _ => log::warn!(target: "host", "Unknown network topic: {}", topic),
        }
    }
}

pub fn register_services(ctx: Arc<HostContext>) {
    let network_service = Arc::new(NetworkService::new(ctx.clone()));
    ctx.dispatcher.register_subscription("network/fs_download".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(network_service.clone()));
    ctx.dispatcher.register_subscription("network/http".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(network_service.clone()));
    ctx.dispatcher.register_subscription("system/task_cancel_broadcast".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(network_service));
}
