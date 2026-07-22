//! Запись файла (топик fs/write): данные берутся из общей памяти по хендлу,
//! результат — событием write_result.

use super::State;
use veldmap_host_util::Access;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsWriteRequest, FsWriteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use std::fs;

pub fn on_write(state: &State, req: FsWriteRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsWriteResult { error: "Access denied".into(), correlation_id }
    } else {
        let handle = match req.handle {
            Some(h) => h,
            None => {
                bus::emit::on_write_result(&*state.ctx.dispatcher, &FsWriteResult { error: "Missing handle".into(), correlation_id });
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
                    let path = resolve_path(&state.ctx, &req.path);
                    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
                    match fs::write(&path, &data) {
                        Ok(()) => FsWriteResult { error: String::new(), correlation_id },
                        Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
                    }
                }
                Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
            }
        };
        data
    };
    bus::emit::on_write_result(&*state.ctx.dispatcher, &result);
}
