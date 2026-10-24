use std::collections::{BTreeSet, HashMap, HashSet};

use crate::module::{ReadPurpose, SidecarWrite, State};
use crate::module::storage::{self, LocalFile, OriginSidecar};
use crate::proto::data_library::{
    LibraryEntry, LibraryRequest, LibraryState, LibraryStatus, SnapshotFiles,
};
use crate::proto::data_provider::{ProductRoots, ProductRootsRequest};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsReadRequest, FsWriteRequest, FsWriteResult};

const SIDECAR_CAP: u64 = 64 * 1024;

pub fn on_list(state: &mut State, _req: LibraryRequest) {
    crate::emit::resend::on_state();
    rescan(state);
}

pub fn rescan(state: &mut State) {
    if !state.pending_list.is_empty() {
        state.list_again = true;
        return;
    }
    let correlation_id = state.pending_list.begin(());
    crate::calls::fs::on_list(&FsListRequest {
        path: storage::DATA_DIR.to_string(),
        recursive: true,
    }, &correlation_id);
}

pub fn on_list_result(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if state.pending_list.take(&veldsdk::correlation()).is_none() { return }
    ingest(state, response);
    crate::module::open::resume(state);
    if std::mem::take(&mut state.list_again) {
        rescan(state);
    }
}

fn ingest(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if !response.error.is_empty() {
        publish_error(&response.error);
        return;
    }

    let mut snapshot: HashMap<String, LocalFile> = HashMap::new();
    for (name, file) in response.entries.iter()
        .filter_map(|e| LocalFile::from_entry(&e.name, e.size, e.modified))
    {
        if file.is_partial || !snapshot.contains_key(&name) {
            snapshot.insert(name, file);
        }
    }
    state.snapshot = snapshot;

    let on_disk: HashSet<String> = response.entries.iter()
        .filter_map(|e| e.name.strip_suffix(storage::ORIGIN_SUFFIX))
        .map(str::to_string)
        .collect();
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

    state.forget_gone_troubles();

    let missing: Vec<String> = on_disk.into_iter()
        .filter(|name| !state.origins.contains_key(name))
        .filter(|name| !state.pending_reads.values()
            .any(|purpose| matches!(purpose, ReadPurpose::Sidecar(n) if n == name)))
        .collect();

    publish(state);

    for name in missing {
        let correlation_id = state.pending_reads.begin(ReadPurpose::Sidecar(name.clone()));
        crate::calls::fs::on_read(&FsReadRequest {
            path: storage::origin_path(&name),
        }, &correlation_id);
    }

    ask_product_roots(state);
}

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

pub fn on_sidecar_read(state: &mut State, name: String, opened: &ResourceOpened) {
    let Some(handle) = &opened.handle else { return };

    let bytes = match veldsdk::resource::read_whole(handle.clone(), SIDECAR_CAP) {
        Ok(bytes) => bytes,
        Err(why) => {
            veldsdk::log::warn!(target: "handlers", "sidecar {} not read: {}", name, why);
            return;
        }
    };
    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return };
    if sidecar.provider != storage::PROVIDER_NAME { return }

    if state.origins.contains_key(&name) {
        return;
    }
    if state.pending_delete.values().any(|path| path == &storage::origin_path(&name)) {
        return;
    }

    state.origins.insert(name, sidecar);
    publish(state);
    ask_product_roots(state);
}

pub fn write_sidecar(state: &mut State, name: &str, sidecar: OriginSidecar) {
    state.origins.insert(name.to_string(), sidecar.clone());

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
        handle: Some(region.into_handle()),
    }, &correlation_id);
}

pub fn on_write_result(state: &mut State, response: FsWriteResult) {
    let Some(write) = state.pending_sidecar_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "sidecar not saved: {}", response.error);
    }
    if let Some(waiting) = state.queued_sidecars.remove(&write.name) {
        write_sidecar(state, &write.name, waiting);
        return;
    }
    if !state.origins.contains_key(&write.name) {
        veldsdk::log::debug!(target: "handlers",
            "sidecar {} written after the entry was deleted — removing it", write.name);
        let path = storage::origin_path(&write.name);
        let correlation_id = state.pending_delete.begin(path.clone());
        crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
    }
}

pub fn delete_data(state: &mut State, name: &str) {
    let path = storage::data_path(name, state.is_partial(name));
    let correlation_id = state.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
}

pub fn delete_entry(state: &mut State, name: &str) {
    state.origins.remove(name);
    state.queued_sidecars.remove(name);
    state.troubles.remove(name);
    let path = storage::origin_path(name);
    let correlation_id = state.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
    delete_data(state, name);
}

pub fn publish(state: &State) {
    crate::emit::on_state(&LibraryState { entries: entries(state), error: String::new() });
}

fn publish_error(error: &str) {
    crate::emit::on_state(&LibraryState { entries: Vec::new(), error: error.to_string() });
}

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
            let total = if dl.total > 0 { dl.total } else { known_total };
            (LibraryStatus::LibDownloading, dl.done, total)
        } else if let Some(file) = file {
            if file.is_partial {
                (LibraryStatus::LibPaused, file.size, known_total)
            } else {
                (LibraryStatus::LibComplete, file.size, file.size)
            }
        } else {
            (LibraryStatus::LibPaused, 0, known_total)
        };

        LibraryEntry {
            identifier: state.identifier_of(name).unwrap_or_default().to_string(),
            modified: file.map(|file| file.modified).unwrap_or(0),
            name: name.to_string(),
            done,
            total,
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
