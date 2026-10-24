//! Catalog snapshot and sidecars: reading the disk and publishing the library state.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::module::{ReadPurpose, SidecarWrite, State};
use crate::module::storage::{self, LocalFile, OriginSidecar};
use crate::proto::data_library::{
    LibraryEntry, LibraryRequest, LibraryState, LibraryStatus, SnapshotFiles,
};
use crate::proto::data_provider::{ProductRoots, ProductRootsRequest};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsReadRequest, FsWriteRequest, FsWriteResult};

/// Cap for the sidecar. The sidecar itself is a json of five fields, a few
/// hundred bytes; the cap is set far above not to leave room for them, but to
/// separate the implausible from the merely unusual: a file of tens of
/// kilobytes under the name `.origin` may still be ours, grown in the future,
/// while a megabyte one cannot.
const SIDECAR_CAP: u64 = 64 * 1024;

/// "Re-read the catalogue". The answer comes not from here but from
/// on_list_result: the disk does not answer instantly, and lying about the
/// catalogue contents from memory is exactly what this service does not do.
pub fn on_list(state: &mut State, _req: LibraryRequest) {
    // The state is requested when it is missing: by a subscriber that has just
    // come up. Meanwhile the broadcast drops what did not change (the topic is
    // declared a snapshot), and since last time it may well be unchanged — then
    // "unchanged" would mean "no answer at all". So the fingerprint is
    // forgotten: the nearest broadcast is guaranteed to go out.
    crate::emit::resend::on_state();
    rescan(state);
}

/// Re-read the catalogue; the result is a broadcast of the state.
///
/// The traversal is recursive over the whole catalogue, and reasons to re-read
/// are as many as files in the snapshot: every completed download and every
/// deletion calls here. A second traversal over a running one would answer the
/// same and cost the same, so while one is in flight the reason accumulates as
/// a flag and turns into exactly one traversal when the running one returns.
/// Postpone, not discard: a disk snapshot taken before the change is not an
/// answer to it.
pub fn rescan(state: &mut State) {
    if !state.pending_list.is_empty() {
        state.list_again = true;
        return;
    }
    let correlation_id = state.pending_list.begin(());
    // Deep: a snapshot is a folder, and its files are catalogue entries on par
    // with those in the root. Folders themselves are not entries, and the
    // recursive listing does not return them (see `FsListRequest.recursive`).
    crate::calls::fs::on_list(&FsListRequest {
        path: storage::DATA_DIR.to_string(),
        recursive: true,
    }, &correlation_id);
}

/// The disk snapshot has arrived. Sidecars are trimmed here as well, and those
/// not yet in memory are read back.
pub fn on_list_result(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if state.pending_list.take(&veldsdk::correlation()).is_none() { return }
    ingest(state, response);
    // The catalogue is read — those who asked earlier now have their answer.
    crate::module::open::resume(state);
    // The reasons that piled up while the traversal ran become exactly one
    // traversal. Here, not in ingest: that has exit branches, and each would
    // have to remember the postponed reason anew.
    if std::mem::take(&mut state.list_again) {
        rescan(state);
    }
}

fn ingest(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if !response.error.is_empty() {
        publish_error(&response.error);
        return;
    }

    // Folded by name, not a file list: `foo` and `foo.part` are one entry.
    // `.part` wins: it says the entry is being worked on now, while the file
    // under the same name is left from a previous attempt and will be replaced
    // by it. The pair appears only outside the app — a re-download removes the
    // completed file before starting (see download::on_download) — but the
    // choice must still be named here, not fall out of the catalogue traversal
    // order.
    let mut snapshot: HashMap<String, LocalFile> = HashMap::new();
    for (name, file) in response.entries.iter()
        .filter_map(|e| LocalFile::from_entry(&e.name, e.size, e.modified))
    {
        if file.is_partial || !snapshot.contains_key(&name) {
            snapshot.insert(name, file);
        }
    }
    state.snapshot = snapshot;

    // origins is a cache of the disk, not an independent truth: trim it to what
    // is actually in the catalogue, otherwise a file deleted outside the app
    // would remain an intent record until the end of the session. The exception
    // is sidecars whose write is still in flight: they are regularly absent
    // from the disk, and cutting them would lose the record of a just-started
    // download.
    // A set, not a list: the trimming below asks it about every known name,
    // and on a library of thousands of entries a linear search over a list of
    // the same length would stall the module on every disk traversal.
    let on_disk: HashSet<String> = response.entries.iter()
        .filter_map(|e| e.name.strip_suffix(storage::ORIGIN_SUFFIX))
        .map(str::to_string)
        .collect();
    // Not only what is already on the disk is kept. The sidecar write goes as
    // its own file-system job, the listing as its own, and there is no order
    // between them: a listing that left before the write does not see the
    // sidecar, yet its answer may well arrive later. So names whose sidecar is
    // in flight stay alive as well, and so do names of ongoing downloads —
    // their origin is ours all the more.
    let in_flight: Vec<&str> = state.pending_sidecar_writes.values()
        .map(|w| w.name.as_str())
        .chain(state.queued_sidecars.keys().map(String::as_str))
        .collect();
    let downloading: Vec<&str> = state.downloads.values().map(|dl| dl.name.as_str()).collect();
    state.origins.retain(|name, _| {
        on_disk.contains(name.as_str())
            || in_flight.contains(&name.as_str())
            || downloading.contains(&name.as_str())
    });

    // Failure reasons follow the entries.
    state.forget_gone_troubles();

    // Read back what is not yet in memory. The sidecar is the only thing that
    // survives a restart, so for files from a previous run it is the only way
    // to learn the provider key and the expected size.
    let missing: Vec<String> = on_disk.into_iter()
        .filter(|name| !state.origins.contains_key(name))
        .filter(|name| !state.pending_reads.values()
            .any(|purpose| matches!(purpose, ReadPurpose::Sidecar(n) if n == name)))
        .collect();

    // The state is published right away, without waiting for the sidecars: the
    // files on the disk are already the truth, and the origin is read back and
    // arrives with the next broadcast.
    publish(state);

    for name in missing {
        let correlation_id = state.pending_reads.begin(ReadPurpose::Sidecar(name.clone()));
        crate::calls::fs::on_read(&FsReadRequest {
            path: storage::origin_path(&name),
        }, &correlation_id);
    }

    ask_product_roots(state);
}

/// Asks the provider which snapshots the entries without a recorded snapshot
/// belong to. These are files downloaded outside the app or before the
/// boundary began to be recorded — on their own they fall out of their
/// snapshot.
///
/// Ask, do not derive: the snapshot boundary is the layout of the provider's
/// bucket, and a second carrier of it would diverge from the first on the very
/// first new mission. The answer goes into the sidecar: it is the only thing
/// that survives a restart anyway, and there is no point in asking the same on
/// every listing.
fn ask_product_roots(state: &mut State) {
    let identifiers: Vec<String> = state
        .origins
        .values()
        .filter(|origin| origin.product.is_empty() && !origin.identifier.is_empty())
        .map(|origin| origin.identifier.clone())
        .collect();
    if identifiers.is_empty() {
        return;
    }
    let correlation_id = state.pending_roots.begin();
    crate::calls::data_provider::on_product_roots(
        &ProductRootsRequest { identifiers },
        &correlation_id,
    );
}

/// The snapshot roots have arrived. Attribute them to the entries and rewrite
/// the sidecars — after that each sidecar answers on its own which snapshot the
/// file belongs to.
///
/// A key lying in no snapshot is absent from the answer altogether; such an
/// entry stays on its own, and we will ask about it again — it is cheap (an
/// answer without the network) and heals itself if the bucket layout is
/// refined.
pub fn on_product_roots_result(state: &mut State, response: ProductRoots) {
    let correlation_id = veldsdk::correlation();
    if state.pending_roots.settle(&correlation_id) != veldsdk::Reply::Current { return }

    let found: Vec<(String, OriginSidecar)> = state
        .origins
        .iter()
        .filter(|(_, origin)| origin.product.is_empty())
        .filter_map(|(name, origin)| {
            let root = response.roots.get(&origin.identifier)?;
            Some((name.clone(), OriginSidecar { product: root.clone(), ..origin.clone() }))
        })
        .collect();
    if found.is_empty() {
        return;
    }
    for (name, sidecar) in found {
        write_sidecar(state, &name, sidecar);
    }
    publish(state);
}

/// "The snapshot has this many files" — the only thing about a snapshot that
/// cannot be derived from the disk: how many of its files are complete is
/// visible from the entries, while how many there should be is known only to
/// whoever traversed the snapshot in the storage.
///
/// It goes into the sidecars of its files — by the same device as the snapshot
/// root (see [`on_product_roots_result`]): the sidecar is the only thing that
/// survives a restart anyway, and there is no point in asking the same on
/// every listing. Only those that do not know this number yet or know a
/// different one are rewritten: the snapshot is traversed more than once, and
/// an extra write to disk is an extra write to disk.
pub fn on_snapshot(state: &mut State, request: SnapshotFiles) {
    if request.product.is_empty() || request.files == 0 {
        return;
    }
    let stale: Vec<(String, OriginSidecar)> = state
        .origins
        .iter()
        .filter(|(_, origin)| origin.product == request.product)
        .filter(|(_, origin)| origin.siblings != request.files)
        .map(|(name, origin)| {
            (name.clone(), OriginSidecar { siblings: request.files, ..origin.clone() })
        })
        .collect();
    if stale.is_empty() {
        return;
    }
    for (name, sidecar) in stale {
        write_sidecar(state, &name, sidecar);
    }
    publish(state);
}

/// The sidecar of entry `name` has been read (the wait is already settled, see
/// module::on_read_result). A missing sidecar is not an error: the file could
/// have been downloaded outside the app, there is simply nothing to resume it
/// with.
pub fn on_sidecar_read(state: &mut State, name: String, opened: &ResourceOpened) {
    let Some(handle) = &opened.handle else { return };

    // The sidecar is a few dozen bytes of json that we write ourselves;
    // anything bigger under this name is not our file (a truncated download,
    // foreign junk).
    let bytes = match veldsdk::resource::read_whole(handle.clone(), SIDECAR_CAP) {
        Ok(bytes) => bytes,
        Err(why) => {
            veldsdk::log::warn!(target: "handlers", "sidecar {} not read: {}", name, why);
            return;
        }
    };
    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return };
    if sidecar.provider != storage::PROVIDER_NAME { return }

    // What was read from the disk is the oldest known about the entry: its
    // reading started with the listing, and while it was reading, more could be
    // learned about the entry. So it only SUPPLEMENTS memory, not overwrites
    // it: otherwise a fresh sidecar from a started download would roll back to
    // the old one, and would even go back to the disk in that form with the
    // first progress.
    if state.origins.contains_key(&name) {
        return;
    }
    // And it does not resurrect the deleted: the sidecar deletion is tracked,
    // and the read may have left before it.
    if state.pending_delete.values().any(|path| path == &storage::origin_path(&name)) {
        return;
    }

    state.origins.insert(name, sidecar);
    publish(state);
    ask_product_roots(state);
}

/// Writes the sidecar. It goes to disk BEFORE the download starts, so it
/// survives a failure that happened before the first bytes appeared.
///
/// The sidecar arrives whole, not field by field: it is written over the old
/// one, and editing a single fact is `OriginSidecar { field: new, ..old }`.
/// Assembled from fields on the spot it would wipe everything the caller did
/// not think of.
pub fn write_sidecar(state: &mut State, name: &str, sidecar: OriginSidecar) {
    state.origins.insert(name.to_string(), sidecar.clone());

    // Two writes of the same sidecar at once are not rare but the usual end of
    // a download: the last file of the snapshot writes its origin via
    // `on_download`, and the whole snapshot follows with `siblings`. Handed to
    // the file system together, they go as two independent jobs, and the rename
    // wins the one that finished last — so the first may stay on disk while the
    // second is in memory. That is why the second waits for the answer to the
    // first; only the last of them waits — the sidecar is written whole, and
    // the disk needs no intermediate states.
    if state.pending_sidecar_writes.values().any(|write| write.name == name) {
        state.queued_sidecars.insert(name.to_string(), sidecar);
        return;
    }

    let Ok(json) = serde_json::to_vec(&sidecar) else {
        veldsdk::log::warn!(target: "handlers", "sidecar {} did not serialize to json", name);
        return;
    };
    let region = match veldsdk::resource::region_of(&json) {
        Ok(region) => region,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "sidecar {} not written: {}", name, error);
            return;
        }
    };
    let correlation_id = state.pending_sidecar_writes.begin(SidecarWrite {
        region: region.id(),
        name: name.to_string(),
    });
    crate::calls::fs::on_write(&FsWriteRequest {
        path: storage::origin_path(name),
        // Ownership goes to the waiting table: on_write_result releases it.
        handle: Some(region.into_handle()),
    }, &correlation_id);
}

/// fs has read our buffer (successfully or not) — the region is no longer
/// needed.
pub fn on_write_result(state: &mut State, response: FsWriteResult) {
    let Some(write) = state.pending_sidecar_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "sidecar not saved: {}", response.error);
    }
    // The slot is free — write what was waiting its turn (see
    // [`write_sidecar`]). The waiting sidecar is fresher than the written one:
    // it is what should stay on disk.
    if let Some(waiting) = state.queued_sidecars.remove(&write.name) {
        write_sidecar(state, &write.name, waiting);
        return;
    }
    // The entry is no longer in memory — it was deleted while the write was
    // going to the file system. There is no way to cancel the dispatched job,
    // so the sidecar is removed once more: otherwise it stays an orphan, and
    // the nearest listing resurrects an intent row from it. The `origins`
    // trimming does not touch an entry with a sidecar in flight, so "not in
    // memory" here means exactly a deletion (see `rescan` and
    // [`delete_entry`]).
    if !state.origins.contains_key(&write.name) {
        veldsdk::log::debug!(target: "handlers",
            "sidecar {} written after the entry was deleted — removing it", write.name);
        let path = storage::origin_path(&write.name);
        let correlation_id = state.pending_delete.begin(path.clone());
        crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
    }
}

/// Deletes the entry's data, keeping the sidecar: this is how a re-download
/// removes the completed file without losing where it came from.
pub fn delete_data(state: &mut State, name: &str) {
    let path = storage::data_path(name, state.is_partial(name));
    let correlation_id = state.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
}

/// Deletes the entry whole — together with its sidecar, otherwise `.origin`
/// would stay an orphan and the file would resurrect as an intent record.
pub fn delete_entry(state: &mut State, name: &str) {
    state.origins.remove(name);
    state.queued_sidecars.remove(name);
    // The failure reason leaves with the entry: it is about the entry, not the
    // name.
    state.troubles.remove(name);
    // The sidecar deletion is tracked, like the data deletion: a `.origin`
    // that failed to delete stays an orphan, and the nearest listing
    // resurrects an intent record from it. A silent failure here means a row in
    // "Downloads" that never goes away, and a reason that exists nowhere.
    let path = storage::origin_path(name);
    let correlation_id = state.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
    delete_data(state, name);
}

// ── Publishing the state ────────────────────────────────────────────

/// Broadcasts the library state — the only way to tell about it.
pub fn publish(state: &State) {
    crate::emit::on_state(&LibraryState { entries: entries(state), error: String::new() });
}

fn publish_error(error: &str) {
    crate::emit::on_state(&LibraryState { entries: Vec::new(), error: error.to_string() });
}

/// Builds the entries from three sources. None of them is a superset of the
/// rest: a download may run before the file appears, and a sidecar may stay
/// without data.
///
/// The names are collected as a set, not a list with membership checks: a name
/// is the key of an entry, and a second time it would mean a second entry of
/// the same file. Ordered — the entry order comes out as well, nothing to
/// sort.
fn entries(state: &State) -> Vec<LibraryEntry> {
    let names: BTreeSet<&str> = state.snapshot.keys()
        .chain(state.origins.keys())
        .map(String::as_str)
        .chain(state.downloads.values().map(|dl| dl.name.as_str()))
        .collect();

    names.into_iter().map(|name| {
        let known_total = state.total_bytes(name);
        let file = state.entry_for(name);

        let (status, done, total) = if let Some((_, dl)) = state.active_download(name) {
            // While the download is alive, the bytes come only from here: the
            // disk snapshot updates only on terminal events and is knowingly
            // stale during the download.
            let total = if dl.total > 0 { dl.total } else { known_total };
            (LibraryStatus::LibDownloading, dl.done, total)
        } else if let Some(file) = file {
            if file.is_partial {
                (LibraryStatus::LibPaused, file.size, known_total)
            } else {
                (LibraryStatus::LibComplete, file.size, file.size)
            }
        } else {
            // Sidecar present, data absent — a user intent that must not
            // silently vanish from the list.
            (LibraryStatus::LibPaused, 0, known_total)
        };

        LibraryEntry {
            identifier: state.identifier_of(name).unwrap_or_default().to_string(),
            modified: file.map(|file| file.modified).unwrap_or(0),
            name: name.to_string(),
            done,
            total,
            // The reason is carried only by a stopped entry: for a running one
            // the start of the attempt cleared it, and for a completed one it
            // would mean that what arrived failed.
            trouble: match status {
                LibraryStatus::LibPaused => state.troubles.get(name).cloned().unwrap_or_default(),
                _ => String::new(),
            },
            status: status as i32,
            product: state.product_of(name).unwrap_or_default().to_string(),
            siblings: state.siblings_of(name),
        }
    }).collect()
}
