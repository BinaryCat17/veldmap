//! Листинг каталога (топик fs/list): результат — событием list_result.

use super::State;
use veldmap_host_util::bindings::fs as bus;
use veldmap_host_util::bindings::proto::fs::{FsEntry, FsListRequest, FsListResult};
use veldmap_host_util::path::{is_path_safe, resolve_path};
use veldmap_host_util::{blocking, Caller};
use std::fs;
use std::path::Path;

/// Насколько глубоко заходит обход. Не про здравый смысл раскладки, а про то,
/// что символическая ссылка на предка закольцевала бы обход: типом мы её
/// отсеиваем, но предел дешевле и надёжнее одного условия.
const MAX_DEPTH: usize = 32;

pub fn on_list(state: &State, req: FsListRequest, caller: Caller) {
    let correlation = caller.correlation;
    if !is_path_safe(&req.path) {
        bus::emit::on_list_result(&*state.ctx.publisher, &FsListResult {
            entries: vec![], error: "Путь за пределами дозволенного".into(),
        }, &correlation);
        return;
    }

    blocking(&state.ctx, move |ctx| {
        let path = resolve_path(&ctx, &req.path);
        let mut entries = Vec::new();
        let result = match path.exists() {
            false => FsListResult { entries, error: String::new() },
            true => match collect(&path, "", req.recursive, 0, &mut entries) {
                Ok(()) => FsListResult { entries, error: String::new() },
                Err(e) => FsListResult { entries: vec![], error: e.to_string() },
            },
        };
        bus::emit::on_list_result(&*ctx.publisher, &result, &correlation);
    });
}

/// Один уровень каталога, а при `recursive` — и всё, что под ним. `prefix` —
/// путь этого уровня от того, о котором спрашивали; он же становится началом
/// имени записи.
fn collect(
    dir: &Path,
    prefix: &str,
    recursive: bool,
    depth: usize,
    entries: &mut Vec<FsEntry>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        // .part (см. network::download) отдаём как есть — вызывающая сторона
        // (data-browser) сама решает, как показать недокачанное; здесь это
        // просто ещё одно имя файла.
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let name = match prefix.is_empty() {
            true => name,
            false => format!("{}/{}", prefix, name),
        };

        // Тип берём у самой записи, а не через metadata: она идёт по ссылке, и
        // каталог за ссылкой мы обходить не хотим — там и кольцо, и чужое
        // поддерево.
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            if recursive && depth < MAX_DEPTH {
                collect(&entry.path(), &name, recursive, depth + 1, entries)?;
            }
            // Плоский листинг отдаёт каталог обычной записью — нулевого
            // размера, как его видит файловая система; рекурсивный не отдаёт
            // вовсе (см. `FsListRequest.recursive`).
            if recursive {
                continue;
            }
        }

        // Размер и время — из одного metadata: два вызова читают каталог
        // дважды и могут разойтись.
        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
        let modified = metadata
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_secs() as i64)
            .unwrap_or(0);
        entries.push(FsEntry { name, size, modified });
    }
    Ok(())
}
