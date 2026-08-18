//! Удаление файла (топик fs/delete): результат — событием delete_result.
//! Используется data-browser для явного удаления недокачанных (.part)
//! файлов с экрана Downloaded.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsDeleteRequest, FsDeleteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};
use std::fs;

pub fn on_delete(state: &State, req: FsDeleteRequest, caller: Caller) {
    let correlation = caller.correlation;
    if !is_path_safe(&req.path) {
        bus::emit::on_delete_result(&*state.ctx.publisher, &FsDeleteResult {
            error: "Путь за пределами дозволенного".into(),
        }, &correlation);
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let path = resolve_path(&ctx, &req.path);
        let result = match fs::remove_file(&path) {
            Ok(()) => {
                prune_empty(path.parent(), &ctx.config.runtime_dir);
                FsDeleteResult { error: String::new() }
            }
            // Удалять то, чего нет — не ошибка: цель вызова достигнута.
            // Нужно для строк-намерений в data-browser (.origin без данных
            // на диске, см. handlers::nav) и для sidecar'ов у файлов,
            // скачанных до его появления. Проверяем kind, а не текст ошибки:
            // сборка кросс-компилируется под Windows, где текст другой.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                FsDeleteResult { error: String::new() }
            }
            Err(e) => FsDeleteResult { error: e.to_string() },
        };
        bus::emit::on_delete_result(&*ctx.publisher, &result, &correlation);
    });
}

/// Убирает каталоги, опустевшие после удаления файла, — снизу вверх, пока они
/// пусты и пока не дошли до корня runtime.
///
/// Здесь, а не у заказчика: раскладку он свою знает, но «каталог опустел» —
/// свойство файловой системы, а не его учёта, и увидеть его можно только
/// отсюда. Опасности в этом нет по устройству: `remove_dir` сносит только
/// пустой каталог и на первом же непустом останавливает подъём.
fn prune_empty(from: Option<&std::path::Path>, root: &std::path::Path) {
    let mut dir = from;
    while let Some(path) = dir {
        if path == root || !path.starts_with(root) || fs::remove_dir(path).is_err() {
            return;
        }
        dir = path.parent();
    }
}
