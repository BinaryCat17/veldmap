//! Чтение файла (топик fs/read): содержимое кладётся в общую память,
//! результат — событием read_result с хендлом ресурса.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsReadRequest, FsReadResult};
use veldmap_host_util::core::ResourceHandle;
use veldmap_host_util::path::is_path_safe;
use std::fs;

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
