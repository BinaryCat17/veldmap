//! Eviction: keeps the cache within its budget by deleting whole oldest sources.
//!
//! The sweep runs in two beats of listings: the root (source names) → the
//! source directories (files, sizes, marker mtime). The decision is the pure
//! function [`victims`]; the deletion is file by file, fire-and-forget. The
//! victim's directory needs no removal: `fs` prunes it once it runs empty,
//! walking up to the runtime root — and if it does not, the next fill of the
//! same source reuses the name.
//!
//! Races with live queries are safe by construction: a file deleted from under
//! a read is a miss, and a miss is a rebuild. But a source touched within this
//! window is barred from being a victim (see [`victims`]) — marker freshness
//! is not enough for that: the marker reaches the filesystem as a separate
//! job, and the sweep listing can well read it before the marker is rewritten.
//! Then the very source being looked at right now would look like the oldest.

use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsListResult, FsDeleteResult};

use crate::module::{layout, State};

/// How often to re-check the budget: once per this many store events. The
/// first sweep runs on the very first store after startup: earlier sessions
/// may not live long enough to reach a sweep of their own.
const SWEEP_EVERY_STORES: u32 = 256;

/// How far below the budget to sweep: evicting right at the limit would be
/// triggered by every following tile.
const HEADROOM_DIVISOR: u64 = 10;

/// The sweep in progress. A second one never runs in parallel (see maybe_sweep).
pub struct Sweep {
    /// Source directory listings in flight.
    pub pending: u32,
    pub sources: Vec<Source>,
}

pub struct Source {
    pub key: String,
    pub bytes: u64,
    /// Freshness: the marker's mtime; without a marker — the freshest file. Unix seconds.
    pub used: i64,
    /// File names — the victim is deleted file by file.
    pub files: Vec<String>,
}

/// What was asked of fs/on_list.
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
    // Touched sources are forgotten along with the sweep: the marker of any
    // source that receives at least one tile after this is rewritten (see
    // `store::touch`), so it arrives at the next sweep fresh.
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
            // Everything in the root with a valid name is a source; the rest
            // is not ours and is left alone.
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
            // A failed listing does not make a source a victim: unknown
            // contents are not "empty and old" — there is nothing to delete.
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

/// End of the sweep: total the bytes, pick victims, dispatch the deletions.
fn settle(state: &mut State, sweep: Sweep) {
    let total: u64 = sweep.sources.iter().map(|s| s.bytes).sum();
    let within = within(total, state.limit_bytes);
    let doomed = victims(sweep.sources, total, state.limit_bytes, &state.touched);
    if doomed.is_empty() {
        match within {
            true => veldsdk::log::debug!(target: "handlers",
                "cache within budget: {} of {} bytes", total, state.limit_bytes),
            // The budget is over, but there is nothing to evict: everything
            // on disk was touched within this window. It still must be said —
            // the cache grows beyond the budget meanwhile — but into the
            // analysis log: the state lasts while the display runs, and in the
            // console it would repeat at every sweep.
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

/// Whether the used space fits the budget. One function for both askers —
/// the eviction decision and the line about it: diverging, they would tell
/// different stories about the cache.
fn within(total: u64, limit: u64) -> bool {
    total <= limit
}

/// The eviction decision: the oldest sources, until there is room again.
/// A pure function — the rule is fixed by the code, not left to the runs.
///
/// `alive` — sources touched within this sweep window. They are never
/// victims, whatever the freshness says: freshness is measured by the marker
/// on disk, and by this moment the marker may not be written yet — the
/// filesystem job is one thing, the sweep listing another, and there is no
/// order between them. Otherwise the very source whose tiles are being asked
/// for right now would go under the knife.
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
