use crate::dispatcher::NativeService;
use crate::resources::{ResourceManager, Resource};
use crate::services::{
    FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, 
    FsDownloadRequest, LogRequest, TaskResponse, TaskStatusRequest, TaskStatusResponse,
    TaskCancelRequest, GpuResourceRequest, GpuResourceResponse, ResourceHandle
};
use prost::Message;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::task::AbortHandle;

struct TaskState {
    progress: f32,
    completed: bool,
    error: String,
    abort_handle: Option<AbortHandle>,
}

pub struct SystemService {
    tasks: Arc<Mutex<HashMap<String, TaskState>>>,
    resources: Arc<ResourceManager>,
}

impl SystemService {
    pub fn new(resources: Arc<ResourceManager>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            resources,
        }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match method {
            "create_resource" => {
                let req = GpuResourceRequest::decode(&payload[..])?;
                let mut handle = ResourceHandle::default();
                match req.command {
                    Some(crate::services::gpu_resource_request::Command::CreateTexture(t)) => {
                        handle.id = self.resources.create_texture(t.width, t.height, t.format, t.usage);
                        handle.size = (t.width * t.height * 4) as u64; 
                        handle.r#type = 1; // ResourceType::Texture
                    }
                    Some(crate::services::gpu_resource_request::Command::CreateBuffer(b)) => {
                        handle.id = self.resources.create_buffer(b.size, b.usage);
                        handle.size = b.size;
                        handle.r#type = 0; // ResourceType::Buffer
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported resource command")),
                }
                Ok(GpuResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            "fs_read" => {
                let req = FsReadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                
                let metadata = fs::metadata(&req.path)?;
                let size = metadata.len();
                
                // Zero-copy чтение напрямую в GPU буфер
                let id = self.resources.create_buffer_mapped(size, 0, |view| {
                    use std::io::Read;
                    let mut file = fs::File::open(&req.path)?;
                    file.read_exact(view)?;
                    Ok(())
                })?;
                
                let handle = ResourceHandle {
                    id,
                    size,
                    r#type: 0,
                    content_hash: self.resources.compute_hash(id).unwrap_or_default(),
                };
                Ok(FsReadResponse { handle: Some(handle) }.encode_to_vec())
            }
            "fs_write" => {
                let req = FsWriteRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let handle = req.handle.ok_or_else(|| anyhow::anyhow!("Missing handle"))?;
                
                let data = self.resources.read_resource(handle.id, 0, handle.size)?;

                if let Some(parent) = Path::new(&req.path).parent() { fs::create_dir_all(parent)?; }
                fs::write(&req.path, &data)?;
                Ok(Vec::new())
            }
            "fs_download" => {
                let req = FsDownloadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }

                let task_id = uuid::Uuid::new_v4().to_string();
                
                let tasks_clone = self.tasks.clone();
                if let Some(parent) = Path::new(&req.path).parent() { fs::create_dir_all(parent)?; }
                
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
                            return;
                        }
                    };

                    if !res.status().is_success() {
                        let mut tasks = tasks_clone.lock().unwrap();
                        if let Some(t) = tasks.get_mut(&task_id_inner) {
                            t.error = format!("HTTP {}", res.status());
                            t.completed = true;
                        }
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
                            return;
                        }
                    }

                    let mut tasks = tasks_clone.lock().unwrap();
                    if let Some(t) = tasks.get_mut(&task_id_inner) {
                        t.progress = 1.0;
                        t.completed = true;
                    }
                });

                {
                    let mut tasks = self.tasks.lock().unwrap();
                    tasks.insert(task_id.clone(), TaskState { 
                        progress: 0.0, 
                        completed: false, 
                        error: String::new(),
                        abort_handle: Some(join_handle.abort_handle()),
                    });
                }

                use crate::services::FsDownloadResponse;
                Ok(FsDownloadResponse { 
                    task: Some(TaskResponse { task_id }) 
                }.encode_to_vec())
            }
            "task_status" => {
                let req = TaskStatusRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get(&req.task_id) {
                    let response = TaskStatusResponse { 
                        progress: task.progress, 
                        completed: task.completed, 
                        error: task.error.clone() 
                    }.encode_to_vec();
                    
                    if task.completed {
                        tasks.remove(&req.task_id);
                    }
                    
                    Ok(response)
                } else {
                    Err(anyhow::anyhow!("Task not found"))
                }
            }
            "task_cancel" => {
                let req = TaskCancelRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get_mut(&req.task_id) {
                    if let Some(handle) = task.abort_handle.take() {
                        handle.abort();
                    }
                    task.completed = true;
                    task.error = "Cancelled by user".to_string();
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Task not found"))
                }
            }
            "fs_list" => {
                let req = FsListRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let mut entries = Vec::new();
                if Path::new(&req.path).exists() {
                    for entry in fs::read_dir(&req.path)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() { entries.push(name.to_string()); }
                    }
                }
                Ok(FsListResponse { entries }.encode_to_vec())
            }
            "log" => {
                let req = LogRequest::decode(&payload[..])?;
                log::info!("[WASM] {}", req.message);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown method")),
        }
    }
}
