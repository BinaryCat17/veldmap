//! Запись файла (топик fs/on_write): данные берутся из общей памяти по хендлу,
//! результат — событием on_write_result.
//!
//! Запись атомарна: сначала во временный файл рядом, потом rename поверх.
//! Прямая запись обрезала бы существующий файл до нуля и наполняла его на
//! месте — а любой обрыв в этот момент (убийство, падение, отключение
//! питания) оставлял бы на диске полуфайл, который читатели приняли бы за
//! настоящий. Через rename читатель видит либо старое содержимое целиком,
//! либо новое целиком.

use super::State;
use veldmap_host_util::Access;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsWriteRequest, FsWriteResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};
use std::fs;

pub fn on_write(state: &State, req: FsWriteRequest, caller: Caller) {
    let Caller { instance, correlation, .. } = caller;
    let fail = |error: &str| FsWriteResult { error: error.into() };

    if !is_path_safe(&req.path) {
        bus::emit::on_write_result(&*state.ctx.publisher, &fail("Путь за пределами дозволенного"), &correlation);
        return;
    }
    let Some(handle) = req.handle else {
        bus::emit::on_write_result(&*state.ctx.publisher, &fail("Хендл ресурса не назван"), &correlation);
        return;
    };
    if handle.id == 0 {
        bus::emit::on_write_result(&*state.ctx.publisher, &fail("Хендл 0 в fs/write не поддерживается"), &correlation);
        return;
    }
    if !state.ctx.registry.check_access(handle.id, instance, Access::Read) {
        bus::emit::on_write_result(&*state.ctx.publisher, &fail("Чтение этого ресурса запрещено"), &correlation);
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let result = match ctx.memory.read(handle.id, 0, handle.size) {
            Ok(data) => {
                let path = resolve_path(&ctx, &req.path);
                if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
                match write_atomically(&path, &data) {
                    Ok(()) => FsWriteResult { error: String::new() },
                    Err(e) => FsWriteResult { error: e.to_string() },
                }
            }
            Err(e) => FsWriteResult { error: e.to_string() },
        };
        bus::emit::on_write_result(&*ctx.publisher, &result, &correlation);
    });
}

/// Пишет во временный файл рядом с целевым и переименовывает поверх.
///
/// Рядом, а не в системный temp: rename атомарен только в пределах одной
/// файловой системы, а через границу он превратился бы в копирование — то
/// есть ровно в то неатомарное наполнение на месте, от которого мы уходим.
///
/// Имя временного — по id операции, а не «.tmp»: две записи в один каталог
/// идут параллельно и общим именем затёрли бы друг друга.
fn write_atomically(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let tmp = std::path::PathBuf::from(tmp);

    if let Err(e) = fs::write(&tmp, data) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
