//! Реализация сервиса fs (контракт — veldcore/proto/fs.schema.yaml).
//! Свободные обработчики on_input_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).

use veldmap_host_util::{Access, HostContext};
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{
    FsReadRequest, FsReadResult, FsWriteRequest, FsWriteResult,
    FsListRequest, FsListResult,
};
use veldmap_host_util::core::ResourceHandle;
use veldmap_host_util::path::is_path_safe;
use std::sync::Arc;
use std::path::Path;
use std::fs;

pub struct State {
    ctx: Arc<HostContext>,
}

pub fn init(ctx: Arc<HostContext>) -> State {
    State { ctx }
}

pub fn on_input_read(state: &State, req: FsReadRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsReadResult { handle: None, error: "Access denied".into(), correlation_id }
    } else {
        match fs::read(&req.path) {
            Ok(data) => {
                let size = data.len() as u64;
                let id = state.ctx.memory.alloc_cpu(data, requestor_id);
                let handle = ResourceHandle {
                    id,
                    size,
                    content_hash: state.ctx.memory.compute_hash(id).unwrap_or_default(),
                };
                FsReadResult { handle: Some(handle), error: String::new(), correlation_id }
            }
            Err(e) => FsReadResult { handle: None, error: e.to_string(), correlation_id },
        }
    };
    bus::emit::read_result(&*state.ctx.dispatcher, &result);
}

pub fn on_input_write(state: &State, req: FsWriteRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsWriteResult { error: "Access denied".into(), correlation_id }
    } else {
        let handle = match req.handle {
            Some(h) => h,
            None => {
                bus::emit::write_result(&*state.ctx.dispatcher, &FsWriteResult { error: "Missing handle".into(), correlation_id });
                return;
            }
        };

        let data = if handle.id == 0 {
            FsWriteResult { error: "Handle ID 0 not supported for fs_write yet".into(), correlation_id }
        } else if !state.ctx.registry.check_access(handle.id, requestor_id, Access::Read) {
            FsWriteResult { error: "Access denied to resource".into(), correlation_id }
        } else {
            match state.ctx.memory.read(handle.id, 0, handle.size) {
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
    bus::emit::write_result(&*state.ctx.dispatcher, &result);
}

pub fn on_input_list(state: &State, req: FsListRequest, _requestor_id: u32) {
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
    bus::emit::list_result(&*state.ctx.dispatcher, &result);
}
