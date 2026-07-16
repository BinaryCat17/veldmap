#![recursion_limit = "256"]
use veldmap_host_core::dispatcher::AsyncNativeService;
use veldmap_host_core::registry::Access;
use veldmap_host_core::core::{
    FsReadRequest, FsReadResult, FsWriteRequest, FsWriteResult,
    FsListRequest, FsListResult, ResourceHandle
};
use prost::Message;
use std::sync::Arc;
use std::path::Path;
use std::fs;

use veldmap_host_core::setup::HostContext;

pub struct FsService {
    ctx: Arc<HostContext>,
}

impl FsService {
    pub fn new(ctx: Arc<HostContext>) -> Self {
        Self { ctx }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }

    async fn handle_fs_read(&self, payload: Vec<u8>, requestor_id: u32) {
        let req = match FsReadRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode FsReadRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        let result = if !Self::is_path_safe(&req.path) {
            FsReadResult { handle: None, error: "Access denied".into(), correlation_id }
        } else {
            match fs::read(&req.path) {
                Ok(data) => {
                    let size = data.len() as u64;
                    let id = self.ctx.memory.alloc_cpu(data, requestor_id);
                    let handle = ResourceHandle {
                        id,
                        size,
                        content_hash: self.ctx.memory.compute_hash(id).unwrap_or_default(),
                    };
                    FsReadResult { handle: Some(handle), error: String::new(), correlation_id }
                }
                Err(e) => FsReadResult { handle: None, error: e.to_string(), correlation_id },
            }
        };
        self.ctx.dispatcher.publish("fs/read_result", result.encode_to_vec());
    }

    async fn handle_fs_write(&self, payload: Vec<u8>, requestor_id: u32) {
        let req = match FsWriteRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode FsWriteRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        let result = if !Self::is_path_safe(&req.path) {
            FsWriteResult { error: "Access denied".into(), correlation_id }
        } else {
            let handle = match req.handle {
                Some(h) => h,
                None => {
                    self.ctx.dispatcher.publish("fs/write_result", FsWriteResult { error: "Missing handle".into(), correlation_id }.encode_to_vec());
                    return;
                }
            };
            
            let data = if handle.id == 0 {
                FsWriteResult { error: "Handle ID 0 not supported for fs_write yet".into(), correlation_id }
            } else if !self.ctx.registry.check_access(handle.id, requestor_id, Access::Read) {
                FsWriteResult { error: "Access denied to resource".into(), correlation_id }
            } else {
                match self.ctx.memory.read(handle.id, 0, handle.size) {
                    Ok(data) => {
                        if let Some(parent) = Path::new(&req.path).parent() { let _ = fs::create_dir_all(parent); }
                        match fs::write(&req.path, &data) {
                            Ok(()) => FsWriteResult { error: String::new(), correlation_id },
                            Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
                        }
                    }
                    Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
                }
            };
            data
        };
        self.ctx.dispatcher.publish("fs/write_result", result.encode_to_vec());
    }

    async fn handle_fs_list(&self, payload: Vec<u8>) {
        let req = match FsListRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode FsListRequest: {}", e);
                return;
            }
        };

        let correlation_id = req.correlation_id.clone();
        let result = if !Self::is_path_safe(&req.path) {
            FsListResult { entries: vec![], error: "Access denied".into(), correlation_id }
        } else {
            let mut entries = Vec::new();
            if Path::new(&req.path).exists() {
                match fs::read_dir(&req.path) {
                    Ok(iter) => {
                        for entry in iter {
                            if let Ok(entry) = entry {
                                if let Some(name) = entry.file_name().to_str() { entries.push(name.to_string()); }
                            }
                        }
                        FsListResult { entries, error: String::new(), correlation_id }
                    }
                    Err(e) => FsListResult { entries: vec![], error: e.to_string(), correlation_id },
                }
            } else {
                FsListResult { entries: vec![], error: String::new(), correlation_id }
            }
        };
        self.ctx.dispatcher.publish("fs/list_result", result.encode_to_vec());
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for FsService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32) {
        match topic {
            "read" => self.handle_fs_read(payload, requestor_id).await,
            "write" => self.handle_fs_write(payload, requestor_id).await,
            "list" => self.handle_fs_list(payload).await,
            _ => log::warn!(target: "host", "Unknown fs topic: {}", topic),
        }
    }
}

pub fn register_services(ctx: Arc<HostContext>) {
    let fs_service = Arc::new(FsService::new(ctx.clone()));
    ctx.dispatcher.register_subscription("fs/read".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(fs_service.clone()));
    ctx.dispatcher.register_subscription("fs/write".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(fs_service.clone()));
    ctx.dispatcher.register_subscription("fs/list".to_string(), veldmap_host_core::dispatcher::ServiceLocation::NativeAsync(fs_service));
}
