//! Удаление файла (топик fs/delete): результат — событием delete_result.
//! Используется data-browser для явного удаления недокачанных (.part)
//! файлов с экрана Downloaded.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsDeleteRequest, FsDeleteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use std::fs;

pub fn on_delete(state: &State, req: FsDeleteRequest, _requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsDeleteResult { error: "Access denied".into(), correlation_id }
    } else {
        let path = resolve_path(&state.ctx, &req.path);
        match fs::remove_file(&path) {
            Ok(()) => FsDeleteResult { error: String::new(), correlation_id },
            Err(e) => FsDeleteResult { error: e.to_string(), correlation_id },
        }
    };
    bus::emit::on_delete_result(&*state.ctx.dispatcher, &result);
}
