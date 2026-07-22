#![recursion_limit = "256"]
//! Контракт сервиса — veldcore/proto/fs.proto + fs.schema.yaml; типы и стабы
//! топиков — из сгенерированных биндингов (platform/host/generated).

use veldmap_host_util::{AsyncNativeService, Access, HostContext};
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{
    FsReadRequest, FsReadResult, FsWriteRequest, FsWriteResult,
    FsListRequest, FsListResult,
};
use veldmap_host_util::core::ResourceHandle;
use veldmap_host_util::path::is_path_safe;
use veldmap_host_util::wire;
use std::sync::Arc;
use std::path::Path;
use std::fs;

pub struct FsService {
    ctx: Arc<HostContext>,
}

impl FsService {
    pub fn new(ctx: Arc<HostContext>) -> Self {
        Self { ctx }
    }

    async fn handle_fs_read(&self, payload: Vec<u8>, requestor_id: u32) {
        let Some(req) = wire::decode_or_log::<FsReadRequest>(&payload, bus::topics::READ) else { return };

        let correlation_id = req.correlation_id.clone();
        let result = if !is_path_safe(&req.path) {
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
        bus::emit::read_result(&*self.ctx.dispatcher, &result);
    }

    async fn handle_fs_write(&self, payload: Vec<u8>, requestor_id: u32) {
        let Some(req) = wire::decode_or_log::<FsWriteRequest>(&payload, bus::topics::WRITE) else { return };

        let correlation_id = req.correlation_id.clone();
        let result = if !is_path_safe(&req.path) {
            FsWriteResult { error: "Access denied".into(), correlation_id }
        } else {
            let handle = match req.handle {
                Some(h) => h,
                None => {
                    bus::emit::write_result(&*self.ctx.dispatcher, &FsWriteResult { error: "Missing handle".into(), correlation_id });
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
        bus::emit::write_result(&*self.ctx.dispatcher, &result);
    }

    async fn handle_fs_list(&self, payload: Vec<u8>) {
        let Some(req) = wire::decode_or_log::<FsListRequest>(&payload, bus::topics::LIST) else { return };

        let correlation_id = req.correlation_id.clone();
        let result = if !is_path_safe(&req.path) {
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
        bus::emit::list_result(&*self.ctx.dispatcher, &result);
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
    wire::subscribe_async(&ctx, &fs_service, &[bus::topics::READ, bus::topics::WRITE, bus::topics::LIST]);
}
