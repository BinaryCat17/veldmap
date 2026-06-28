use veldmap_host_core::dispatcher::{NativeService, TaskState};
use veldmap_host_core::registry::{ResourceRegistry, Access, ResourceBackend};
use veldmap_host_core::memory::MemoryManager;
use veldmap_host_core::graphics::GraphicsDevice;
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
    ctx: Arc<veldmap_host_core::setup::HostContext>,
    tasks: Arc<Mutex<HashMap<String, TaskState>>>,
    configs: Arc<DashMap<u32, HashMap<String, serde_json::Value>>>,
}

impl SystemService {
    pub fn new(ctx: Arc<veldmap_host_core::setup::HostContext>) -> Self {
        Self {
            ctx,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            configs: Arc::new(DashMap::new()),
        }
    }

    pub fn register_config(&self, instance_id: u32, config: HashMap<String, serde_json::Value>) {
        self.configs.insert(instance_id, config);
    }

    pub fn unregister_config(&self, instance_id: u32) {
        self.configs.remove(&instance_id);
    }

    pub fn has_tasks(&self) -> bool {
        !self.tasks.lock().unwrap().is_empty()
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
                if let Some(id) = self.ctx.registry.get_named_id(&req.name) {
                    if self.ctx.registry.check_access(id, requestor_id, Access::Read) {
                        let mut handle = ResourceHandle { id, ..Default::default() };
                        if let Some(ResourceBackend::Memory) = self.ctx.registry.get_backend(id) {
                            handle.size = self.ctx.memory.get_size(id);
                        } else {
                            handle.size = 0;
                        }
                        Ok(GetResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
                    } else {
                        Ok(GetResourceResponse { handle: None, error: "Access Denied".into() }.encode_to_vec())
                    }
                } else {
                    Ok(GetResourceResponse { handle: None, error: format!("Resource '{}' not found", req.name) }.encode_to_vec())
                }
            }
            "create_data" => {
                let req = CreateDataRequest::decode(&payload[..])?;
                let id = self.ctx.memory.alloc_cpu(vec![0u8; req.size as usize], requestor_id);
                let handle = ResourceHandle {
                    id,
                    size: req.size,
                    content_hash: Vec::new(),
                };
                Ok(CreateDataResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            "task_create" => {
                let req = TaskCreateRequest::decode(&payload[..])?;
                let task_id = if req.task_id.is_empty() {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    req.task_id.clone()
                };
                let mut tasks = self.tasks.lock().unwrap();
                tasks.insert(task_id.clone(), TaskState {
                    progress: 0.0,
                    completed: false,
                    error: String::new(),
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
                
                // Broadcast cancellation to all services
                self.ctx.dispatcher.publish("system/task_cancel_broadcast", payload);

                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get_mut(&req.task_id) {
                    task.completed = true;
                    task.error = "Cancelled by user".to_string();
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Task not found"))
                }
            }
            _ => Err(anyhow::anyhow!("Unknown system method")),
        }
    }
}

pub fn register_services(ctx: Arc<veldmap_host_core::setup::HostContext>) -> Arc<SystemService> {
    let service = Arc::new(SystemService::new(ctx.clone()));
    ctx.dispatcher.register_service("system".to_string(), veldmap_host_core::dispatcher::ServiceLocation::Native(service.clone()));
    service
}
