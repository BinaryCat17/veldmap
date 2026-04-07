use crate::dispatcher::NativeService;
use crate::resources::{ResourceManager, Resource};
use crate::core::{
    TaskStatusRequest, TaskStatusResponse,
    TaskCancelRequest, ResourceHandle,
    GetResourceRequest, GetResourceResponse, CreateDataRequest, CreateDataResponse
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

pub struct SystemService {
    tasks: Arc<Mutex<HashMap<String, crate::dispatcher::TaskState>>>,
    resources: Arc<ResourceManager>,
}

impl SystemService {
    pub fn new(resources: Arc<ResourceManager>, tasks: Arc<Mutex<HashMap<String, crate::dispatcher::TaskState>>>) -> Self {
        Self {
            tasks,
            resources,
        }
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
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
            "task_status" => {
                let req = TaskStatusRequest::decode(&payload[..])?;
                let mut tasks = self.tasks.lock().unwrap();
                if let Some(task) = tasks.get(&req.task_id) {
                    let response = TaskStatusResponse { 
                        progress: task.progress, 
                        completed: task.completed, 
                        error: task.error.clone(),
                        result_handle: task.result_handle.clone(),
                        payload: task.payload.clone(),
                    }.encode_to_vec();
                    
                    if task.completed {
                        log::debug!(target: "host", "Task {} completed and removed from host", req.task_id);
                        tasks.remove(&req.task_id);
                    }
                    
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
                use crate::core::AcquireResourceRequest;
                let req = AcquireResourceRequest::decode(&payload[..])?;
                if self.resources.acquire_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found or unauthorized", req.id))
                }
            }
            "release_resource" => {
                use crate::core::ReleaseResourceRequest;
                let req = ReleaseResourceRequest::decode(&payload[..])?;
                self.resources.release_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            "freeze_resource" => {
                use crate::core::FreezeResourceRequest;
                let req = FreezeResourceRequest::decode(&payload[..])?;
                if self.resources.freeze_resource(req.id, requestor_id) {
                    Ok(Vec::new())
                } else {
                    Err(anyhow::anyhow!("Resource {} not found to freeze or unauthorized", req.id))
                }
            }
            "destroy_resource" => {
                use crate::core::DestroyResourceRequest;
                let req = DestroyResourceRequest::decode(&payload[..])?;
                self.resources.destroy_resource(req.id, requestor_id);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown system method")),
        }
    }
}
