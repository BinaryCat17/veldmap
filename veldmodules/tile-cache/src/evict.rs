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
//! файл — это промах, промах — это пересборка. А вот источник, который трогали
//! в этом окне, в жертвы не попадает запретом (см. [`victims`]) — свежести
//! маркера для этого мало: маркер отдаётся файловой системе отдельным
//! заданием, и листинг обхода вполне читает его раньше, чем тот перезапишется.
//! Тогда источник, на который сейчас смотрят, выглядит старейшим.

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
    // Отмеченные забываются вместе с обходом: маркер источника, в который
    // после этого положат хоть один тайл, перепишется заново (см.
    // `store::touch`), и в следующий обход он придёт свежим.
    state.touched.clear();
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
    let within = within(total, state.limit_bytes);
    let doomed = victims(sweep.sources, total, state.limit_bytes, &state.touched);
    if doomed.is_empty() {
        match within {
            true => veldsdk::log::debug!(target: "handlers",
                "кэш в бюджете: {} из {} байт", total, state.limit_bytes),
            // Бюджет перебран, а вытеснять нечего: всё, что лежит, трогали в
            // этом окне. Сказать надо — кэш при этом растёт сверх бюджета, —
            // но в журнал разбора: состояние держится, пока идёт показ, и в
            // консоли повторялось бы каждым обходом.
            false => veldsdk::log::debug!(target: "handlers",
                "кэш перебрал бюджет ({} из {} байт), но все источники в работе — вытеснять нечего",
                total, state.limit_bytes),
        }
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

/// Умещается ли занятое в бюджет. Одной функцией на оба спрашивающих —
/// решение о вытеснении и строку о нём: разойдясь, они рассказывали бы о
/// кэше разное.
fn within(total: u64, limit: u64) -> bool {
    total <= limit
}

/// Решение вытеснения: старейшие источники, пока не станет свободно.
/// Чистая функция — правило закреплено тестом, а не прогонами.
///
/// `alive` — источники, которых трогали в этом окне обхода. Они не жертвы ни
/// при какой свежести: свежесть меряется маркером на диске, а маркер к этому
/// мигу может быть ещё не записан — задание файловой системы своё, листинг
/// обхода своё, и порядка между ними нет. Из-за этого под нож попадал бы ровно
/// тот источник, тайлы которого сейчас и просят.
fn victims(
    mut sources: Vec<Source>,
    total: u64,
    limit: u64,
    alive: &std::collections::HashSet<String>,
) -> Vec<Source> {
    if within(total, limit) {
        return Vec::new();
    }
    let target = limit - limit / HEADROOM_DIVISOR;
    sources.retain(|source| !alive.contains(&source.key));
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

    /// Никого не трогали — обычный случай теста про свежесть.
    fn none() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }

    #[test]
    fn under_budget_keeps_everything() {
        let picked = victims(vec![source("a", 10, 1), source("b", 20, 2)], 30, 100, &none());
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
            &none(),
        );
        let keys: Vec<&str> = picked.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    /// Источник, которого трогали в этом окне, не жертва ни при какой
    /// свежести маркера: маркер мог ещё не записаться, а тайлы у него просят
    /// прямо сейчас.
    #[test]
    fn a_touched_source_is_never_a_victim() {
        let alive: std::collections::HashSet<String> = ["hot".to_string()].into_iter().collect();
        let picked = victims(
            // «hot» самый старый по маркеру и самый большой — и всё равно жив.
            vec![source("hot", 90, 1), source("old", 30, 2)],
            120,
            100,
            &alive,
        );
        let keys: Vec<&str> = picked.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["old"]);
    }

    #[test]
    fn fresh_marker_saves_active_source() {
        // Активный источник (used свежий) выживает, даже если он самый большой.
        let picked = victims(
            vec![source("hot", 90, 100), source("old", 30, 1)],
            120,
            100,
            &none(),
        );
        let keys: Vec<&str> = picked.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["old"]);
    }
}
