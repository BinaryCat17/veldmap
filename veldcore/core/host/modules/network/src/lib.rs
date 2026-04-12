use veldmap_host_core::dispatcher::{AsyncNativeService, Dispatcher};
use veldmap_host_core::core::{
    FsDownloadRequest, HttpTaskRequest, HttpTaskResponse,
    FsDownloadResponse, TaskResponse,
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::Path;
use std::fs;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct NetworkService {
    dispatcher: Arc<Dispatcher>,
    tasks: Arc<Mutex<HashMap<String, veldmap_host_core::dispatcher::TaskState>>>,
}

impl NetworkService {
    pub fn new(dispatcher: Arc<Dispatcher>, tasks: Arc<Mutex<HashMap<String, veldmap_host_core::dispatcher::TaskState>>>) -> Self {
        Self { dispatcher, tasks }
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
                task: Some(TaskResponse { task_id: String::new() }),
                correlation_id: correlation_id.clone(),
            };
            self.dispatcher.publish("network/fs_download_result", response.encode_to_vec());
            return;
        }

        let task_id = uuid::Uuid::new_v4().to_string();
        let correlation_id = if req.correlation_id.is_empty() {
            task_id.clone()
        } else {
            req.correlation_id.clone()
        };
        let tasks_clone = self.tasks.clone();
        let dispatcher_clone = self.dispatcher.clone();
        if let Some(parent) = Path::new(&req.path).parent() { let _ = fs::create_dir_all(parent); }

        {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(task_id.clone(), veldmap_host_core::dispatcher::TaskState { 
                progress: 0.0, 
                completed: false, 
                error: String::new(),
                abort_handle: None,
                result_handle: None,
                payload: Vec::new(),
            });
        }

        let task_id_inner = task_id.clone();
        let join_handle = tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut builder = client.get(&req.url);
            for (key, value) in req.headers { builder = builder.header(key, value); }
            
            let res = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    let mut tasks = tasks_clone.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                        t.error = e.to_string();
                        t.completed = true;
                    }
                    let response = FsDownloadResponse {
                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                        correlation_id: correlation_id.clone(),
                    };
                    dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
                    return;
                }
            };

            if !res.status().is_success() {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_inner) {
                    t.error = format!("HTTP {}", res.status());
                    t.completed = true;
                }
                let response = FsDownloadResponse {
                    task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                    correlation_id: correlation_id.clone(),
                };
                dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
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
                                    let mut tasks = tasks_clone.lock().unwrap();
                                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                                        t.error = format!("Write error: {}", e);
                                        t.completed = true;
                                    }
                                    let response = FsDownloadResponse {
                                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                                        correlation_id: correlation_id.clone(),
                                    };
                                    dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
                                    return;
                                }
                                downloaded += chunk.len() as u64;
                                if total_size > 0 {
                                    let mut tasks = tasks_clone.lock().unwrap();
                                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                                        t.progress = downloaded as f32 / total_size as f32;
                                    }
                                }
                            }
                            Err(e) => {
                                let mut tasks = tasks_clone.lock().unwrap();
                                if let Some(t) = tasks.get_mut(&task_id_inner) {
                                    t.error = format!("Stream error: {}", e);
                                    t.completed = true;
                                }
                                let response = FsDownloadResponse {
                                    task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                                    correlation_id: correlation_id.clone(),
                                };
                                dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
                                return;
                            }
                        }
                    }
                    let _ = async_file.flush().await;
                }
                Err(e) => {
                    let mut tasks = tasks_clone.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                        t.error = format!("File create error: {}", e);
                        t.completed = true;
                    }
                    let response = FsDownloadResponse {
                        task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                        correlation_id: correlation_id.clone(),
                    };
                    dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
                    return;
                }
            }

            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(t) = tasks.get_mut(&task_id_inner) {
                t.progress = 1.0;
                t.completed = true;
            }
            let response = FsDownloadResponse {
                task: Some(TaskResponse { task_id: task_id_inner.clone() }),
                correlation_id: correlation_id.clone(),
            };
            dispatcher_clone.publish("network/fs_download_result", response.encode_to_vec());
        });

        {
            let mut tasks = self.tasks.lock().unwrap();
            if let Some(t) = tasks.get_mut(&task_id) {
                t.abort_handle = Some(join_handle.abort_handle());
            }
        }
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
        let tasks_clone = self.tasks.clone();
        let dispatcher_clone = self.dispatcher.clone();
        let task_id_inner = task_id.clone();
        
        log::info!(target: "host", "Received HTTP request: {} {} (correlation_id: {})", req.method, req.url, correlation_id);

        {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(task_id.clone(), veldmap_host_core::dispatcher::TaskState { 
                progress: 0.0, 
                completed: false, 
                error: String::new(),
                abort_handle: None,
                result_handle: None,
                payload: Vec::new(),
            });
        }

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

            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(t) = tasks.get_mut(&task_id_inner) {
                match result {
                    Ok((status, body)) => {
                        log::info!(target: "host", "HTTP task {} finished with status {}", task_id_inner, status);
                        let response = HttpTaskResponse { status, body, correlation_id: correlation_id.clone() };
                        t.payload = response.encode_to_vec();
                        t.progress = 1.0;
                        t.completed = true;
                        dispatcher_clone.publish("network/http_result", t.payload.clone());
                    }
                    Err(e) => {
                        log::warn!(target: "host", "HTTP task {} failed: {}", task_id_inner, e);
                        let response = HttpTaskResponse { status: 0, body: Vec::new(), correlation_id: correlation_id.clone() };
                        t.error = e;
                        t.completed = true;
                        dispatcher_clone.publish("network/http_result", response.encode_to_vec());
                    }
                }
            }
        });

        if let Some(t) = self.tasks.lock().unwrap().get_mut(&task_id) {
            t.abort_handle = Some(join_handle.abort_handle());
        }
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for NetworkService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, _requestor_id: u32) {
        match topic {
            "fs_download" => self.handle_fs_download(payload).await,
            "http" => self.handle_http(payload).await,
            _ => log::warn!(target: "host", "Unknown network topic: {}", topic),
        }
    }
}
