//! Чтение файла (топик fs/read): в ответе — ресурс, подключённый к файлу.
//!
//! Содержимое не поднимается в память: файл подключается диапазонным
//! носителем (`RangeSource`), тем же, что и удалённый ресурс у network.
//! Поэтому так открываются и файлы, которые в память не влезают, а
//! потребитель тянет нужные фрагменты через resource_read — отдельного
//! ranged-протокола на шине для этого не нужно.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::FsReadRequest;
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, opened, opened_handle, Caller};

pub fn on_read(state: &State, req: FsReadRequest, caller: Caller) {
    let Caller { instance, correlation, .. } = caller;
    if !is_path_safe(&req.path) {
        bus::emit::on_read_result(&*state.ctx.publisher,
            &opened(Err("Access denied".into())), &correlation);
        return;
    }

    // Открытие — тоже обращение к диску: на сетевом или спящем носителе
    // даже open с метаданными отвечает не сразу.
    blocking(&state.ctx, move |ctx| {
        let result = match ctx.memory.alloc_file(&resolve_path(&ctx, &req.path), instance) {
            Ok((id, size)) => opened_handle(id, size),
            Err(e) => opened(Err(e.to_string())),
        };
        bus::emit::on_read_result(&*ctx.publisher, &result, &correlation);
    });
}
