use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsListResult, FsDeleteResult};

use crate::module::{layout, State};

const SWEEP_EVERY_STORES: u32 = 256;

const HEADROOM_DIVISOR: u64 = 10;

pub struct Sweep {
    pub pending: u32,
    pub sources: Vec<Source>,
}

pub struct Source {
    pub key: String,
    pub bytes: u64,
    pub used: i64,
    pub files: Vec<String>,
}

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
                veldsdk::log::warn!(target: "handlers", "cache sweep did not start: {}", result.error);
                state.sweep = None;
                return;
            }
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
                    "{} not swept and left alone this time: {}", key, result.error);
                None
            };

            let finished = {
                let sweep = state.sweep.as_mut().expect("the sweep is running: the wait was ours");
                sweep.sources.extend(source);
                sweep.pending -= 1;
                sweep.pending == 0
            };
            if finished {
                let sweep = state.sweep.take().expect("the sweep was just there");
                settle(state, sweep);
            }
        }
    }
}

fn settle(state: &mut State, sweep: Sweep) {
    let total: u64 = sweep.sources.iter().map(|s| s.bytes).sum();
    let within = within(total, state.limit_bytes);
    let doomed = victims(sweep.sources, total, state.limit_bytes, &state.touched);
    if doomed.is_empty() {
        match within {
            true => veldsdk::log::debug!(target: "handlers",
                "cache within budget: {} of {} bytes", total, state.limit_bytes),
            false => veldsdk::log::debug!(target: "handlers",
                "cache over budget ({} of {} bytes), but every source is in use — nothing to evict",
                total, state.limit_bytes),
        }
        return;
    }

    let freed: u64 = doomed.iter().map(|s| s.bytes).sum();
    veldsdk::log::info!(target: "handlers",
        "eviction: {} bytes of a budget of {}, {} victims taking {} bytes",
        total, state.limit_bytes, doomed.len(), freed);

    for source in doomed {
        for file in source.files {
            let path = format!("{}/{}", layout::source_dir(&source.key), file);
            let correlation = state.pending_deletes.begin(path.clone());
            crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation);
        }
    }
}

fn within(total: u64, limit: u64) -> bool {
    total <= limit
}

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
        veldsdk::log::warn!(target: "handlers", "{} not deleted: {}", path, result.error);
    }
}
