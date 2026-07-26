//! Чтение файла (топик fs/read): в ответе — ресурс, подключённый к файлу.
//!
//! Содержимое не поднимается в память: ресурс читается по смещению
//! (DataBacking::File), поэтому так открываются и файлы, которые в память
//! не влезают. Потребитель тянет нужные фрагменты через arena_read —
//! отдельного ranged-протокола на шине для этого не нужно.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsReadRequest, FsReadResult};
use veldmap_host_util::core::ResourceHandle;
use veldmap_host_util::path::{is_path_safe, resolve_path};

pub fn on_read(state: &State, req: FsReadRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsReadResult { handle: None, error: "Access denied".into(), correlation_id }
    } else {
        match state.ctx.memory.alloc_file(&resolve_path(&state.ctx, &req.path), requestor_id) {
            Ok((id, size)) => FsReadResult {
                handle: Some(ResourceHandle { id, size }),
                error: String::new(),
                correlation_id,
            },
            Err(e) => FsReadResult { handle: None, error: e.to_string(), correlation_id },
        }
    };
    bus::emit::on_read_result(&*state.ctx.dispatcher, &result);
}
