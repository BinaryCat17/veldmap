//! Запись файла (топик fs/write): данные берутся из общей памяти по хендлу,
//! результат — событием write_result.

use super::State;
use veldmap_host_util::Access;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsWriteRequest, FsWriteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};
use std::fs;

pub fn on_write(state: &State, req: FsWriteRequest, caller: Caller) {
    let Caller { instance, correlation } = caller;
    let fail = |error: &str| FsWriteResult { error: error.into() };

    if !is_path_safe(&req.path) {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Access denied"), &correlation);
        return;
    }
    let Some(handle) = req.handle else {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Missing handle"), &correlation);
        return;
    };
    if handle.id == 0 {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Handle ID 0 not supported for fs_write yet"), &correlation);
        return;
    }
    if !state.ctx.registry.check_access(handle.id, instance, Access::Read) {
        bus::emit::on_write_result(&*state.ctx.dispatcher, &fail("Access denied to resource"), &correlation);
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let result = match ctx.memory.read(handle.id, 0, handle.size) {
            Ok(data) => {
                let path = resolve_path(&ctx, &req.path);
                if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
                match fs::write(&path, &data) {
                    Ok(()) => FsWriteResult { error: String::new() },
                    Err(e) => FsWriteResult { error: e.to_string() },
                }
            }
            Err(e) => FsWriteResult { error: e.to_string() },
        };
        bus::emit::on_write_result(&*ctx.dispatcher, &result, &correlation);
    });
}
