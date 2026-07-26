//! Запись файла (топик fs/write): данные берутся из общей памяти по хендлу,
//! результат — событием write_result.

use super::State;
use veldmap_host_util::Access;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsWriteRequest, FsWriteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::blocking;
use std::fs;

pub fn on_write(state: &State, req: FsWriteRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let fail = |error: &str| FsWriteResult { error: error.into(), correlation_id: correlation_id.clone() };

    if !is_path_safe(&req.path) {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Access denied"));
        return;
    }
    let Some(handle) = req.handle else {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Missing handle"));
        return;
    };
    if handle.id == 0 {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Handle ID 0 not supported for fs_write yet"));
        return;
    }
    if !state.ctx.registry.check_access(handle.id, requestor_id, Access::Read) {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Access denied to resource"));
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let result = match ctx.memory.read(handle.id, 0, handle.size) {
            Ok(data) => {
                let path = resolve_path(&ctx, &req.path);
                if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
                match fs::write(&path, &data) {
                    Ok(()) => FsWriteResult { error: String::new(), correlation_id },
                    Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
                }
            }
            Err(e) => FsWriteResult { error: e.to_string(), correlation_id },
        };
        bus::emit::on_write_result(&*ctx.dispatcher, &result);
    });
}
