//! Листинг каталога (топик fs/list): результат — событием list_result.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsEntry, FsListRequest, FsListResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};
use std::fs;

pub fn on_list(state: &State, req: FsListRequest, caller: Caller) {
    let correlation = caller.correlation;
    if !is_path_safe(&req.path) {
        bus::emit::on_list_result(&*state.ctx.dispatcher, &FsListResult {
            entries: vec![], error: "Access denied".into(),
        }, &correlation);
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let path = resolve_path(&ctx, &req.path);
        let mut entries = Vec::new();
        let result = if !path.exists() {
            FsListResult { entries, error: String::new() }
        } else {
            match fs::read_dir(&path) {
                Ok(iter) => {
                    for entry in iter.flatten() {
                        // .part (см. network::download) отдаём как есть — вызывающая
                        // сторона (data-browser) сама решает, как показать недокачанное;
                        // здесь это просто ещё одно имя файла.
                        if let Some(name) = entry.file_name().to_str() {
                            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            entries.push(FsEntry { name: name.to_string(), size });
                        }
                    }
                    FsListResult { entries, error: String::new() }
                }
                Err(e) => FsListResult { entries: vec![], error: e.to_string() },
            }
        };
        bus::emit::on_list_result(&*ctx.dispatcher, &result, &correlation);
    });
}
