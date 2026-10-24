use crate::proto::data_library::{
    DownloadRequest, ItemRequest, LibraryEntry, LibraryState as LibraryStateMsg, SnapshotFiles,
};
use crate::proto::data_provider::{ListPathRequest, ListPathResponse};

use crate::module::state::listing::Chosen;
use crate::module::state::{Batch, Listing, State, ViewId, ViewKind};
use crate::module::state::library::status_of;
use crate::proto::data_library::LibraryStatus;

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

pub fn on_delete_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_delete(&ItemRequest { name });
}

pub fn on_reveal_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_reveal(&ItemRequest { name });
}

pub fn on_download_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let path = crate::module::components::folder_path(&product);
    let correlation_id = state.listings.begin(Listing::Snapshot { product, files: 0, queued: 0 });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: String::new(), recursive: true },
        &correlation_id,
    );
}

pub struct Counted {
    pub product: String,
    pub files: u32,
    pub queued: usize,
}

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

    let short = crate::module::components::last_segment(&product).to_string();
    if response.next_token.is_empty() {
        veldsdk::log::info!(target: "handlers", "snapshot '{}': {} files, {} queued", product, files, queued);
        crate::calls::data_library::on_snapshot(&SnapshotFiles { product, files });
        state.notice = Some(match (files, queued) {
            (0, _) => format!("Snapshot \"{}\" contains no files", short),
            (_, 0) => format!("Snapshot \"{}\" is already on disk", short),
            (_, n) => format!("Snapshot \"{}\": {} files queued", short, n),
        });
        return;
    }
    if queued >= super::browse::MAX_ITEMS {
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

pub fn on_pause_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let going = |entry: &LibraryEntry| status_of(entry) == LibraryStatus::LibDownloading;
    for name in files_of(state, &product, going) {
        crate::calls::data_library::on_pause(&ItemRequest { name });
    }
}

pub fn on_delete_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    for name in files_of(state, &product, |_| true) {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }
}

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

#[derive(PartialEq, Eq, Debug)]
pub enum Fetch {
    Snapshot(String),
    File { identifier: String, product: String },
}

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

fn fetches(state: &State, view: ViewId) -> Vec<Fetch> {
    selection(state, view)
        .iter()
        .filter_map(|(key, what)| fetch_of(state, key, what))
        .collect()
}

fn deletions_of(state: &State, key: &str, what: &Chosen) -> Vec<String> {
    match what {
        Chosen::Snapshot => files_of(state, key, |_| true),
        Chosen::File { identifier, name, .. } => {
            let named = match state.library.by_identifier(identifier) {
                Some(entry) => entry.name.clone(),
                None => name.clone(),
            };
            match named.is_empty() {
                true => Vec::new(),
                false => vec![named],
            }
        }
    }
}

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
    names.sort();
    names.dedup();
    (names, gone)
}

pub fn batch(state: &State, view: ViewId) -> Batch {
    let picked = selection(state, view);
    Batch {
        deletable: picked.iter().any(|(key, what)| !deletions_of(state, key, what).is_empty()),
        fetchable: picked.iter().any(|(key, what)| fetch_of(state, key, what).is_some()),
    }
}

fn selection(state: &State, view: ViewId) -> Vec<(String, Chosen)> {
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else {
        return Vec::new();
    };
    let mut picked: Vec<(String, Chosen)> =
        listing.selected.iter().map(|(key, what)| (key.clone(), what.clone())).collect();
    picked.sort_by(|left, right| left.0.cmp(&right.0));
    picked
}

fn files_of(state: &State, product: &str, wanted: impl Fn(&LibraryEntry) -> bool) -> Vec<String> {
    state
        .library
        .entries
        .iter()
        .filter(|entry| entry.product == product && wanted(*entry))
        .map(|entry| entry.name.clone())
        .collect()
}

pub fn on_state(state: &mut State, msg: LibraryStateMsg) {
    if !msg.error.is_empty() {
        state.error = Some(format!("Library: {}", msg.error));
        return;
    }
    state.error = None;
    state.library.entries = msg.entries;
    measure_speed(state);
}

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
        if seconds > 0 {
            state.speed = done.saturating_sub(measured_done) as f32 / seconds as f32;
            state.measured = Some((now, done));
        }
        return;
    }
    state.measured = Some((now, done));
}
