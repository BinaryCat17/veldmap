//! Library actions: download/pause, delete — and accepting its state.
//!
//! Nothing here knows paths, sidecars, or how an unfinished download is told
//! apart on disk: data-library does all of that. The module only turns a press
//! into a request to it, and gets state by broadcast.

use crate::proto::data_library::{
    DownloadRequest, ItemRequest, LibraryEntry, LibraryState as LibraryStateMsg, SnapshotFiles,
};
use crate::proto::data_provider::{ListPathRequest, ListPathResponse};

use crate::module::state::listing::Chosen;
use crate::module::state::{Batch, Listing, State, ViewId, ViewKind};
use crate::module::state::library::status_of;
use crate::proto::data_library::LibraryStatus;

/// The user pressed "download". Pressing again on an ongoing download is a
/// pause: what is downloaded stays, and the next press resumes from it.
pub fn on_download_pressed(state: &mut State, identifier: String, product: String) {
    if identifier.is_empty() { return; }

    let downloading = state.library.by_identifier(&identifier)
        .filter(|e| status_of(e) == LibraryStatus::LibDownloading)
        .map(|e| e.name.clone());

    if let Some(name) = downloading {
        crate::calls::data_library::on_pause(&ItemRequest { name });
        return;
    }

    crate::calls::data_library::on_download(&DownloadRequest { identifier, product });
}

/// The user pressed "delete" — on any library record (complete, unfinished,
/// or declared by intent alone). This also cancels an ongoing download; there
/// is no separate "cancel", because it must leave behind the same thing —
/// nothing (see the row menu in components::table).
pub fn on_delete_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_delete(&ItemRequest { name });
}

/// Show the record in the file manager. The library computes the path: the
/// storage layout is its concern, and there is no need for us to know it.
pub fn on_reveal_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_reveal(&ItemRequest { name });
}

/// Download the whole snapshot. The library downloads file by file — it does
/// not know about snapshots at all — so we first ask the provider what the
/// snapshot consists of.
///
/// By a recursive listing, not a walk by tiers: .SAFE files are laid out in
/// four or five levels, and a walk would cost a request per level.
pub fn on_download_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let path = crate::module::components::folder_path(&product);
    let correlation_id = state.listings.begin(Listing::Snapshot { product, files: 0, queued: 0 });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: String::new(), recursive: true },
        &correlation_id,
    );
}

/// The snapshot and what has been counted for it across pages: the walk goes
/// page by page, while both numbers are running totals.
pub struct Counted {
    pub product: String,
    /// How many snapshot files have been listed so far.
    pub files: u32,
    /// How many of them are queued for download.
    pub queued: usize,
}

/// The snapshot files arrived — queue for download what is not yet on disk,
/// under the snapshot they were called from.
///
/// A recursive listing never contains folders (see `ListPathRequest.recursive`),
/// so there is nothing to filter by kind: everything that arrived is files.
pub fn on_snapshot_files(
    state: &mut State,
    counted: Counted,
    correlation_id: String,
    response: ListPathResponse,
) {
    let Counted { product, mut files, mut queued } = counted;
    if !response.error.is_empty() {
        state.notice = Some(format!("Snapshot \"{}\" could not be listed: {}", product, response.error));
        return;
    }

    files += response.entries.len() as u32;
    for entry in response.entries {
        // Already finished files are left alone. Re-downloading removes the
        // finished file before starting (see data-library::download), so
        // "resume the snapshot" on a snapshot half of which is already on disk
        // would erase exactly what is in it — and download the gigabytes
        // again. Re-downloading happens file by file, from its menu, where it
        // is marked as irreversible.
        let done = state
            .library
            .by_identifier(&entry.key)
            .is_some_and(|record| status_of(record) == LibraryStatus::LibComplete);
        if done {
            continue;
        }
        crate::calls::data_library::on_download(&DownloadRequest {
            identifier: entry.key,
            product: product.clone(),
        });
        queued += 1;
    }

    // Not the last page — read on with the same correlation. The cap is the
    // same as for a folder walk: a snapshot is finite, but a layout in which
    // it never ends must not turn into an endless download. It cannot be
    // aborted silently — an unfinished snapshot looks no different from a
    // whole one.
    let short = crate::module::components::last_segment(&product).to_string();
    if response.next_token.is_empty() {
        veldsdk::log::info!(target: "handlers", "snapshot '{}': {} files, {} queued", product, files, queued);
        // The walk reached the end — so the snapshot's contents are known in
        // full, and this is the only moment when they can be named. Without
        // this number the library treats as complete any snapshot where
        // everything downloaded is finished — three files out of twenty-six
        // would read as "on disk".
        crate::calls::data_library::on_snapshot(&SnapshotFiles { product, files });
        state.notice = Some(match (files, queued) {
            // The walk ended without meeting a single file. "Already on disk"
            // would be a lie here: there is nothing on disk, and nothing for
            // the storage to download either.
            (0, _) => format!("Snapshot \"{}\" contains no files", short),
            (_, 0) => format!("Snapshot \"{}\" is already on disk", short),
            (_, n) => format!("Snapshot \"{}\": {} files queued", short, n),
        });
        return;
    }
    if queued >= super::browse::MAX_ITEMS {
        // The snapshot's contents are not named here: the walk is aborted, and
        // what was listed is not the whole snapshot. Telling the library the
        // counted number would declare complete exactly the snapshot we did
        // not finish reading.
        state.notice =
            Some(format!("Snapshot \"{}\": {} files queued, the rest skipped", short, queued));
        return;
    }

    let path = crate::module::components::folder_path(&product);
    state.listings.insert(correlation_id.clone(), Listing::Snapshot { product, files, queued });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: response.next_token, recursive: true },
        &correlation_id,
    );
}

/// Pause the whole snapshot download.
///
/// It unfolds the same way as on deletion: the library knows nothing of
/// snapshots, and one can hold nearly thirty files, and pressing pause on each
/// is work the application must do itself. Only ongoing downloads are paused:
/// a pause on a finished file would mean "start and stop".
pub fn on_pause_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let going = |entry: &LibraryEntry| status_of(entry) == LibraryStatus::LibDownloading;
    for name in files_of(state, &product, going) {
        crate::calls::data_library::on_pause(&ItemRequest { name });
    }
}

/// Discard the whole snapshot. The library knows nothing of snapshots — it
/// keeps track of files — so we unfold it here, in the same place where the
/// snapshot row is assembled (`downloaded_rows`), and ask to delete each of
/// its files.
///
/// A separate "delete everything for a snapshot" is deliberately absent from
/// the contract: it would require the library to know the snapshot boundary,
/// while the provider knows it, and a second carrier of this fact would
/// diverge from the first.
pub fn on_delete_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    for name in files_of(state, &product, |_| true) {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }
}

/// Download everything selected in the list.
///
/// Not the same as pressing "download" on every row: there it is a toggle and
/// a second press pauses the download (see [`on_download_pressed`]), while a
/// batch action is named by one word and must do one thing.
pub fn on_download_selected(state: &mut State, view: ViewId) {
    for what in fetches(state, view) {
        match what {
            Fetch::Snapshot(product) => on_download_snapshot(state, product),
            Fetch::File { identifier, product } => {
                crate::calls::data_library::on_download(&DownloadRequest { identifier, product });
            }
        }
    }
}

/// What goes to the download queue on a single batch press.
///
/// Snapshot and file are separated by kind, not by a pair of fields: the
/// library knows nothing of snapshots — the provider is first asked what one
/// consists of (see [`on_download_snapshot`]) — while a file goes straight to
/// the download queue.
#[derive(PartialEq, Eq, Debug)]
pub enum Fetch {
    Snapshot(String),
    File { identifier: String, product: String },
}

/// Whether this row is worth downloading — and how.
///
/// A file is judged precisely: finished and already ongoing ones are skipped,
/// an interrupted one is resumed. A snapshot only when finished as a whole,
/// because what is missing in it is known to the provider, not to us: until a
/// snapshot is walked, the library has no list of its files either. Hence
/// `files == 0` — an unwalked one is worth downloading, and this is the most
/// common case: a snapshot from the network catalog.
fn fetch_of(state: &State, key: &str, what: &Chosen) -> Option<Fetch> {
    match what {
        Chosen::Snapshot => {
            let (done, files) = state.library.snapshot(key);
            (files == 0 || done < files as usize).then(|| Fetch::Snapshot(key.to_string()))
        }
        Chosen::File { identifier, product, .. } => {
            if identifier.is_empty() {
                return None;
            }
            let busy = state.library.by_identifier(identifier).is_some_and(|entry| {
                matches!(
                    status_of(entry),
                    LibraryStatus::LibDownloading | LibraryStatus::LibComplete
                )
            });
            (!busy).then(|| Fetch::File {
                identifier: identifier.clone(),
                product: product.clone(),
            })
        }
    }
}

/// What of the selection is worth downloading. From here on only requests remain.
fn fetches(state: &State, view: ViewId) -> Vec<Fetch> {
    selection(state, view)
        .iter()
        .filter_map(|(key, what)| fetch_of(state, key, what))
        .collect()
}

/// The library record names that deleting this row will unfold into.
///
/// A snapshot unfolds into its files (see [`files_of`]) — the library keeps
/// track of files and knows nothing of snapshots. A file names itself, but the
/// name is asked of the library, not taken from the selection: the file could
/// have been downloaded after it was chosen — then the record appeared, while
/// the selection holds no name. The name remembered in the selection stays for
/// the case when the library has no record under this key at all.
fn deletions_of(state: &State, key: &str, what: &Chosen) -> Vec<String> {
    match what {
        Chosen::Snapshot => files_of(state, key, |_| true),
        Chosen::File { identifier, name, .. } => {
            let named = match state.library.by_identifier(identifier) {
                Some(entry) => entry.name.clone(),
                None => name.clone(),
            };
            // An empty name never reaches the library: there is nothing to
            // delete, and a request without a name it answers with a refusal.
            match named.is_empty() {
                true => Vec::new(),
                false => vec![named],
            }
        }
    }
}

/// Discard everything selected — from disk and from the download queue.
///
/// The selection is cleared only for what actually went to deletion, and only
/// for files: the row for a deleted file is gone from "Downloaded", and the
/// leftover selection would be counted in the header until the end of the
/// session. A snapshot stays selected — its files are deleted from disk, while
/// the snapshot itself lives in the catalog, and there is still something to
/// do with it.
pub fn on_delete_selected(state: &mut State, view: ViewId) {
    let (names, gone) = deletions(state, view);
    for name in names {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }

    let Some(listing) = state.listing_mut(view) else { return };
    for key in gone {
        listing.selected.remove(&key);
    }
}

/// What goes to deletion on a single batch press: library record names and
/// selection keys that will stop pointing at anything afterwards.
///
/// As two lists out of one traversal, not two traversals: should they diverge,
/// the selection would be cleared for what was not deleted.
fn deletions(state: &State, view: ViewId) -> (Vec<String>, Vec<String>) {
    let mut names: Vec<String> = Vec::new();
    let mut gone: Vec<String> = Vec::new();
    for (key, what) in selection(state, view) {
        let asked = deletions_of(state, &key, &what);
        if what != Chosen::Snapshot && !asked.is_empty() {
            gone.push(key);
        }
        names.extend(asked);
    }
    // Both a snapshot and its individual file can be selected, and deleting a
    // record twice means the second request misses what no longer exists.
    names.sort();
    names.dedup();
    (names, gone)
}

/// Whether the batch buttons have anything to do. The same analysis as the
/// action itself will use later ([`deletions_of`], [`fetch_of`]): these
/// answers decide whether to show a button, and a second answer to "what will
/// be done" would one day diverge from the first — the button would promise an
/// action and not perform it.
///
/// As a yes/no answer, not a number: the numbers are shown nowhere, and this
/// is asked on every layout frame — the traversal stops at the very first
/// selected item for which something can be done.
pub fn batch(state: &State, view: ViewId) -> Batch {
    let picked = selection(state, view);
    Batch {
        deletable: picked.iter().any(|(key, what)| !deletions_of(state, key, what).is_empty()),
        fetchable: picked.iter().any(|(key, what)| fetch_of(state, key, what).is_some()),
    }
}

/// The selection in the list — as a copy: library requests follow, and they
/// touch the same state.
///
/// In key order, not set order: the order of a set is arbitrary, and the same
/// selection would give requests in one order one time and in another the
/// next — while the header numbers are counted by this same traversal.
fn selection(state: &State, view: ViewId) -> Vec<(String, Chosen)> {
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else {
        return Vec::new();
    };
    let mut picked: Vec<(String, Chosen)> =
        listing.selected.iter().map(|(key, what)| (key.clone(), what.clone())).collect();
    picked.sort_by(|left, right| left.0.cmp(&right.0));
    picked
}

/// The snapshot record names — how the library addresses its files. The rule
/// "what counts as a file of this snapshot" is written here once for all: both
/// pause and deletion unfold a snapshot, and two traversals of its records
/// would one day diverge — and pause would keep downloading what deletion
/// erased.
///
/// As names, not references to records: library requests follow the
/// traversal, and there is no need to keep its state busy until the end.
fn files_of(state: &State, product: &str, wanted: impl Fn(&LibraryEntry) -> bool) -> Vec<String> {
    state
        .library
        .entries
        .iter()
        .filter(|entry| entry.product == product && wanted(*entry))
        .map(|entry| entry.name.clone())
        .collect()
}

/// The library sent state — its own or in answer to our request. We have no
/// second source of truth about what is downloaded, so we simply replace.
///
/// A failure is shown until the next successful state and cleared by it: a
/// message with nobody to clear it hangs on screen until restart and lies the
/// longer the application runs.
pub fn on_state(state: &mut State, msg: LibraryStateMsg) {
    if !msg.error.is_empty() {
        state.error = Some(format!("Library: {}", msg.error));
        return;
    }
    state.error = None;
    state.library.entries = msg.entries;
    measure_speed(state);
}

/// Download speed — from two adjacent states: how many bytes were added over
/// how many seconds. The library has no field for it, and should not have one
/// — it broadcasts what exists now, not how fast it was growing.
///
/// The measurement resets when there is nothing to download: the speed of a
/// finished download is no longer a speed, but the last number seen.
fn measure_speed(state: &mut State) {
    let (count, done, _) = state.library.downloading();
    if count == 0 {
        state.speed = 0.0;
        state.measured = None;
        return;
    }

    let now = crate::module::components::format::now();

    if let Some((measured_at, measured_done)) = state.measured {
        let seconds = now - measured_at;
        // The same instant is not enough: dividing by zero seconds yields not
        // a speed but infinity. The previous measurement is kept — the next
        // state will then be compared against it.
        if seconds > 0 {
            state.speed = done.saturating_sub(measured_done) as f32 / seconds as f32;
            state.measured = Some((now, done));
        }
        return;
    }
    state.measured = Some((now, done));
}
