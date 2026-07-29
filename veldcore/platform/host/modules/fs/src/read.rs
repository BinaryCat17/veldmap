//! Чтение файла (топик fs/read): в ответе — ресурс, подключённый к файлу.
//!
//! Содержимое не поднимается в память: файл подключается диапазонным
//! носителем (`RangeSource`), тем же, что и удалённый ресурс у network.
//! Поэтому так открываются и файлы, которые в память не влезают, а
//! потребитель тянет нужные фрагменты через arena_read — отдельного
//! ranged-протокола на шине для этого не нужно.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::FsReadRequest;
use veldmap_host_util::core::{ResourceHandle, ResourceOpened};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};

pub fn on_read(state: &State, req: FsReadRequest, caller: Caller) {
    let Caller { instance, correlation } = caller;
    if !is_path_safe(&req.path) {
        bus::emit::on_read_result(&*state.ctx.publisher, &ResourceOpened {
            handle: None, error: "Access denied".into(),
        }, &correlation);
        return;
    }

    // Открытие — тоже обращение к диску: на сетевом или спящем носителе
    // даже open с метаданными отвечает не сразу.
    blocking(&state.ctx, move |ctx| {
        let result = match ctx.memory.alloc_file(&resolve_path(&ctx, &req.path), instance) {
            Ok((id, size)) => ResourceOpened {
                handle: Some(ResourceHandle { id, size }),
                error: String::new(),
            },
            Err(e) => ResourceOpened { handle: None, error: e.to_string() },
        };
        bus::emit::on_read_result(&*ctx.publisher, &result, &correlation);
    });
}
