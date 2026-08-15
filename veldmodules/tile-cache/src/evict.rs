//! Вытеснение: держит кэш в бюджете, удаляя старейшие источники целиком.
//!
//! Обход в два такта листингов: корень (имена источников) → каталоги
//! (файлы, размеры, mtime маркера). Решение — чистая функция [`victims`],
//! закреплённая тестом; удаление — файл за файлом, fire-and-forget. Каталог
//! жертвы убирать не надо: опустевший его снимает сам `fs`, дойдя вверх до
//! корня runtime, — а если не снимет, имя переиспользует следующая сборка
//! того же источника.
//!
//! Гонки с живыми запросами безопасны по построению: удалённый из-под чтения
//! файл — это промах, промах — это пересборка. Активный источник в жертвы не
//! попадает не запретом, а свежестью: его маркер только что перезаписан.

use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsListResult, FsDeleteResult};

use crate::module::{layout, State};

/// Как часто перепроверять бюджет: раз в столько store-событий. Первый обход
/// — на первом же store после старта: прошлые сессии могли не дожить до
/// своего обхода.
const SWEEP_EVERY_STORES: u32 = 256;

/// Насколько глубже бюджета вычищать: вытеснение впритык запускалось бы
/// каждым следующим тайлом.
const HEADROOM_DIVISOR: u64 = 10;

/// Идущий обход. Второго параллельно не бывает (см. maybe_sweep).
pub struct Sweep {
    /// Листингов каталогов источников в полёте.
    pub pending: u32,
    pub sources: Vec<Source>,
}

pub struct Source {
    pub key: String,
    pub bytes: u64,
    /// Свежесть: mtime маркера; без маркера — самый свежий файл. Unix-секунды.
    pub used: i64,
    /// Имена файлов — жертву удаляют пофайльно.
    pub files: Vec<String>,
}

/// Что мы спросили у fs/on_list.
pub enum ListPurpose {
    Root,
    Source(String),
}

pub fn maybe_sweep(state: &mut State) {
    if state.sweep.is_some() {
        return;
    }
    if state.swept_once && state.stores_since_sweep < SWEEP_EVERY_STORES {
        return;
    }
    state.swept_once = true;
    state.stores_since_sweep = 0;
    state.sweep = Some(Sweep { pending: 0, sources: Vec::new() });

    let correlation = state.pending_lists.begin(ListPurpose::Root);
    crate::calls::fs::on_list(&FsListRequest { path: layout::ROOT.to_string(), recursive: false }, &correlation);
}

pub fn on_list_result(state: &mut State, result: FsListResult) {
    let Some(purpose) = state.pending_lists.take(&veldsdk::correlation()) else { return };
    if state.sweep.is_none() {
        return;
    }

    match purpose {
        ListPurpose::Root => {
            if !result.error.is_empty() {
                veldsdk::log::warn!(target: "handlers", "обход кэша не начался: {}", result.error);
                state.sweep = None;
                return;
            }
            // Всё в корне с годным именем — источник; прочее не наше и не
            // трогается.
            let keys: Vec<String> = result.entries.iter()
                .filter(|entry| layout::valid_key(&entry.name))
                .map(|entry| entry.name.clone())
                .collect();
            if keys.is_empty() {
                state.sweep = None;
                return;
            }
            if let Some(sweep) = &mut state.sweep {
                sweep.pending = keys.len() as u32;
            }
            for key in keys {
                let correlation = state.pending_lists.begin(ListPurpose::Source(key.clone()));
                crate::calls::fs::on_list(&FsListRequest { path: layout::source_dir(&key), recursive: false }, &correlation);
            }
        }
        ListPurpose::Source(key) => {
            // Неудачный листинг не делает источник жертвой: неизвестное
            // содержимое — это не «пустой и старый», удалять по нему нечего.
            let source = if result.error.is_empty() {
                let mut source = Source { key, bytes: 0, used: 0, files: Vec::new() };
                let mut marker: Option<i64> = None;
                let mut freshest: i64 = 0;
                for entry in &result.entries {
                    source.bytes += entry.size;
                    if entry.name == layout::META {
                        marker = Some(entry.modified);
                    }
                    freshest = freshest.max(entry.modified);
                    source.files.push(entry.name.clone());
                }
                source.used = marker.unwrap_or(freshest);
                Some(source)
            } else {
                veldsdk::log::warn!(target: "handlers",
                    "{} не обойдён и в этот раз не трогается: {}", key, result.error);
                None
            };

            let finished = {
                let sweep = state.sweep.as_mut().expect("обход идёт: ожидание было наше");
                sweep.sources.extend(source);
                sweep.pending -= 1;
                sweep.pending == 0
            };
            if finished {
                let sweep = state.sweep.take().expect("обход только что был");
                settle(state, sweep);
            }
        }
    }
}

/// Конец обхода: посчитать, выбрать жертв, разослать удаления.
fn settle(state: &mut State, sweep: Sweep) {
    let total: u64 = sweep.sources.iter().map(|s| s.bytes).sum();
    let doomed = victims(sweep.sources, total, state.limit_bytes);
    if doomed.is_empty() {
        veldsdk::log::debug!(target: "handlers",
            "кэш в бюджете: {} из {} байт", total, state.limit_bytes);
        return;
    }

    let freed: u64 = doomed.iter().map(|s| s.bytes).sum();
    veldsdk::log::info!(target: "handlers",
        "вытеснение: {} байт при бюджете {}, жертв {} на {} байт",
        total, state.limit_bytes, doomed.len(), freed);

    for source in doomed {
        for file in source.files {
            let path = format!("{}/{}", layout::source_dir(&source.key), file);
            let correlation = state.pending_deletes.begin(path.clone());
            crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation);
        }
    }
}

/// Решение вытеснения: старейшие источники, пока не станет свободно.
/// Чистая функция — правило закреплено тестом, а не прогонами.
fn victims(mut sources: Vec<Source>, total: u64, limit: u64) -> Vec<Source> {
    if total <= limit {
        return Vec::new();
    }
    let target = limit - limit / HEADROOM_DIVISOR;
    sources.sort_by_key(|source| source.used);
    let mut left = total;
    let mut doomed = Vec::new();
    for source in sources {
        if left <= target {
            break;
        }
        left -= source.bytes;
        doomed.push(source);
    }
    doomed
}

pub fn on_delete_result(state: &mut State, result: FsDeleteResult) {
    let Some(path) = state.pending_deletes.take(&veldsdk::correlation()) else { return };
    if !result.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "{} не удалён: {}", path, result.error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(key: &str, bytes: u64, used: i64) -> Source {
        Source { key: key.to_string(), bytes, used, files: Vec::new() }
    }

    #[test]
    fn under_budget_keeps_everything() {
        let picked = victims(vec![source("a", 10, 1), source("b", 20, 2)], 30, 100);
        assert!(picked.is_empty());
    }

    #[test]
    fn evicts_oldest_first_down_to_headroom() {
        // Бюджет 100, занято 150: цель 90, старейшие «a» и «b» дают 150 → 90,
        // и на этом остановка — «c» свежее и выживает.
        let picked = victims(
            vec![source("c", 50, 30), source("a", 40, 10), source("b", 20, 20), source("d", 40, 40)],
            150,
            100,
        );
        let keys: Vec<&str> = picked.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn fresh_marker_saves_active_source() {
        // Активный источник (used свежий) выживает, даже если он самый большой.
        let picked = victims(
            vec![source("hot", 90, 100), source("old", 30, 1)],
            120,
            100,
        );
        let keys: Vec<&str> = picked.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["old"]);
    }
}
