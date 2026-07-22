//! Листинг каталога (топик fs/list): результат — событием list_result.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsListRequest, FsListResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use std::fs;

pub fn on_list(state: &State, req: FsListRequest, _requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();
    let result = if !is_path_safe(&req.path) {
        FsListResult { entries: vec![], error: "Access denied".into(), correlation_id }
    } else {
        let path = resolve_path(&state.ctx, &req.path);
        let mut entries = Vec::new();
        if path.exists() {
            match fs::read_dir(&path) {
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
    bus::emit::on_list_result(&*state.ctx.dispatcher, &result);
}
