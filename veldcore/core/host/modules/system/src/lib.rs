use veldmap_host_core::dispatcher::{NativeService, TaskState};
use veldmap_host_core::resources::{ResourceManager, Resource};
use veldmap_host_core::core::{
    TaskStatusRequest, TaskStatusResponse,
    TaskCancelRequest, TaskCreateRequest, TaskCreateResponse, TaskUpdateRequest, ResourceHandle,
    GetResourceRequest, GetResourceResponse, CreateDataRequest, CreateDataResponse,
    GetConfigRequest, GetConfigResponse, GenerateUuidRequest, GenerateUuidResponse
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use dashmap::DashMap;

pub struct SystemService {
    tasks: Arc<Mutex<HashMap<String, TaskState>>>,
    resources: Arc<ResourceManager>,
    configs: Arc<DashMap<u32, HashMap<String, serde_json::Value>>>,
}

impl SystemService {
    pub fn new(
        resources: Arc<ResourceManager>, 
        tasks: Arc<Mutex<HashMap<String, TaskState>>>
    ) -> Self {
        Self {
            tasks,
            resources,
            configs: Arc::new(DashMap::new()),
        }
    }

    pub fn register_config(&self, instance_id: u32, config: HashMap<String, serde_json::Value>) {
        self.configs.insert(instance_id, config);
    }

    pub fn unregister_config(&self, instance_id: u32) {
        self.configs.remove(&instance_id);
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "get_config" => {
                let req = GetConfigRequest::decode(&payload[..])?;
                let value = if let Some(config) = self.configs.get(&requestor_id) {
                    config.get(&req.key).map(|v| {
                        if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
                    }).unwrap_or_default()
                } else {
                    String::new()
                };
                Ok(GetConfigResponse { value }.encode_to_vec())
            }
            "generate_uuid" => {
                let _req = GenerateUuidRequest::decode(&payload[..])?;
                let uuid = uuid::Uuid::new_v4().to_string();
                Ok(GenerateUuidResponse { uuid }.encode_to_vec())
            }
            "get_resource" => {
                let req = GetResourceRequest::decode(&payload[..])?;
                if let Some(id) = self.resources.get_named_resource(&req.name) {
                    if let Some(res) = self.resources.get_resource(id, requestor_id) {
                        let mut handle = ResourceHandle { id, ..Default::default() };
                        match res {
                            Resource::Data(v) => { handle.size = v.len() as u64; }
                            Resource::Buffer(b) => { handle.size = b.size(); }
                            Resource::Texture { width, height, .. } => { handle.size = (width * height * 4) as u64; }
                            _ => {}
                        }
                        Ok(GetResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
                    } else {
                        Ok(GetResourceResponse { handle: None, error: "Resource found in registry but not in storage or unauthorized".into() }.encode_to_vec())
                    }
                } else {
                    Ok(GetResourceResponse { handle: None, error: format!("Resource '{}' not found", req.name) }.encode_to_vec())
                }
            }
            "create_data" => {
                let req = CreateDataRequest::decode(&payload[..])?;
                let id = self.resources.create_data_resource(vec![0u8; req.size as usize], requestor_id);
                let handle = ResourceHandle {
                    id,
                    size: req.size,
                    content_hash: Vec::new(),
                };
                Ok(CreateDataResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            "task_create" => {
                let _req = TaskCreateRequest::decode(&payload[..])?;
                let task_id = uuid::Uuid::new_v4().to_string();
                let mut tasks = self.tasks.lock().unwrap();
                tasks.insert(task_id.clone(), TaskState {
                    progress: 0.0,
                    completed: false,
                    error: String::new(),
                    abort_handle: None,
                    result_handle: None,
                    payload: Vec::new(),
                });
                Ok(TaskCreateResponse { task_id }.encode_to_vec())
            }
            "task_update" => {
                let req = TaskUpdateRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(t) = tasks.get_mut(&req.task_id) {
                    t.progress = req.progress;
                    t.completed = req.completed;
                    if !req.error.is_empty() { t.error = req.error.clone(); }
                    if !req.payload.is_empty() { t.payload = req.payload.clone(); }
                    
                    // Log task completion or errors
                    if req.completed {
                        if req.error.is_empty() {
                            veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_DISPATCHER, "[TASK] Task {} completed (progress={:.1}%)", req.task_id, req.progress * 100.0);
                        } else {
                            veldmap_host_core::verror!(veldmap_host_core::logging::FLAG_DISPATCHER, "[TASK] Task {} failed: {}", req.task_id, req.error);
                        }
                    } else if !req.error.is_empty() {
                        veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_DISPATCHER, "[TASK] Task {} error: {}", req.task_id, req.error);
                    }
                    
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Task not found"))
                }
            }
            "task_status" => {
                let req = TaskStatusRequest::decode(&payload[..])?;
                let tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get(&req.task_id) {
                    let response = TaskStatusResponse { 
                        progress: task.progress, 
                        completed: task.completed, 
                        error: task.error.clone(),
                        result_handle: task.result_handle.clone(),
                        payload: task.payload.clone(),
                    }.encode_to_vec();
                    
                    Ok(response)
                } else {
                    Err(anyhow::anyhow!("Task {} not found on host", req.task_id))
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
            "acquire_resource" => {
                use veldmap_host_core::core::AcquireResourceRequest;
                let req = AcquireResourceRequest::decode(&payload[..])?;
                if self.resources.acquire_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found or unauthorized", req.id))
                }
            }
            "release_resource" => {
                use veldmap_host_core::core::ReleaseResourceRequest;
                let req = ReleaseResourceRequest::decode(&payload[..])?;
                self.resources.release_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            "freeze_resource" => {
                use veldmap_host_core::core::FreezeResourceRequest;
                let req = FreezeResourceRequest::decode(&payload[..])?;
                if self.resources.freeze_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found to freeze or unauthorized", req.id))
                }
            }
            "destroy_resource" => {
                use veldmap_host_core::core::DestroyResourceRequest;
                let req = DestroyResourceRequest::decode(&payload[..])?;
                self.resources.destroy_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown system method")),
        }
    }
}
