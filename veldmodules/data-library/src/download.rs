use crate::module::{Download, State};
use crate::module::storage;
use crate::module::catalog::{self, delete_data, delete_entry, write_sidecar};
use crate::proto::data_library::{DownloadRequest, ItemRequest};
use crate::proto::data_provider::{SignRequest, SignedUrl};
use veldsdk::proto::network::{FsDownloadProgress, FsDownloadRequest, FsDownloadResponse};

const AT_ONCE: usize = 10;

pub fn on_download(state: &mut State, req: DownloadRequest) {
    if req.identifier.is_empty() { return; }

    let product = match req.product.is_empty() {
        true => state
            .origins
            .values()
            .find(|origin| origin.identifier == req.identifier)
            .map(|origin| origin.product.clone())
            .unwrap_or_default(),
        false => req.product.clone(),
    };
    let name = storage::name_from_identifier(&req.identifier, &product);

    if state.downloads.values().any(|d| d.name == name) {
        veldsdk::log::info!(target: "handlers", "{} is already downloading, ignoring the repeat", name);
        return;
    }

    state.troubles.remove(&name);

    let (total_bytes, siblings) = state.origins.get(&name)
        .map_or((None, 0), |origin| (origin.total_bytes, origin.siblings));
    write_sidecar(state, &name, storage::OriginSidecar {
        provider: storage::PROVIDER_NAME.to_string(),
        identifier: req.identifier.clone(),
        total_bytes,
        product: product.clone(),
        siblings,
    });

    let done = state.entry_for(&name).filter(|file| file.is_partial).map(|file| file.size).unwrap_or(0);
    let total = state.total_bytes(&name);

    let correlation_id = veldsdk::generate_id();
    let queued = state.queue_tick;
    state.queue_tick += 1;
    let waiting = busy(state) >= AT_ONCE;
    state.downloads.insert(correlation_id.clone(), Download {
        identifier: req.identifier.clone(),
        name,
        done,
        total,
        delete_when_done: false,
        queued,
        waiting,
        loading: false,
    });

    if !waiting {
        start(state, &correlation_id);
    }
    catalog::publish(state);
}

/// Puts an entry into motion: clears `waiting` and asks for the signature — the
/// only place a download starts, so `waiting` and "not yet signed" are one fact.
/// A finished file is deleted here, when its queued replacement really starts,
/// not at the press: until then it is still the user's only data for the entry.
fn start(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.get_mut(id) else { return };
    dl.waiting = false;
    let (identifier, name) = (dl.identifier.clone(), dl.name.clone());

    if state.entry_for(&name).is_some_and(|file| !file.is_partial) {
        delete_data(state, &name);
    }
    // A signature that never answers holds the slot: there is no task to kill
    // and no end event will come — only a pause takes the entry off the books.
    crate::calls::data_provider::on_sign(&SignRequest { identifier }, id);
}

fn busy(state: &State) -> usize {
    state.downloads.values().filter(|dl| !dl.waiting).count()
}

fn start_waiting(state: &mut State) {
    if busy(state) >= AT_ONCE {
        return;
    }
    let next = state
        .downloads
        .iter()
        .filter(|(_, dl)| dl.waiting)
        .min_by_key(|(_, dl)| dl.queued)
        .map(|(id, _)| id.clone());
    let Some(correlation_id) = next else { return };
    start(state, &correlation_id);
}

pub fn on_signed(state: &mut State, signed: SignedUrl) {
    let correlation_id = veldsdk::correlation();
    let Some(dl) = state.downloads.get(&correlation_id) else { return };
    let name = dl.name.clone();

    let refusal = veldsdk::reply::undelivered(&signed.error)
        .or_else(|| (!signed.error.is_empty()).then(|| signed.error.clone()));
    if let Some(why) = refusal {
        veldsdk::log::warn!(target: "handlers", "signature for {} failed: {}", name, why);
        state.troubles.insert(name, format!("signature failed: {}", why));
        finish(state, &correlation_id);
        return;
    }

    crate::calls::network::on_fs_download(&FsDownloadRequest {
        url: signed.url,
        path: storage::file_path(&name),
        headers: signed.headers,
    }, &correlation_id);
    if let Some(dl) = state.downloads.get_mut(&correlation_id) {
        dl.loading = true;
    }
}

/// Pause: the `.part` stays where the break left it. Before the request reached
/// the network (`loading`) no end event will come, so the entry is finished
/// here; after that its own result closes it.
pub fn on_pause(state: &mut State, req: ItemRequest) {
    let Some((task_id, dl)) = state.active_download(&req.name) else { return };
    let (task_id, loading) = (task_id.to_string(), dl.loading);
    crate::cancel::network::on_fs_download(&task_id);
    if !loading {
        finish(state, &task_id);
    }
}

pub fn on_delete(state: &mut State, req: ItemRequest) {
    if let Some((task_id, _)) = state.active_download(&req.name) {
        let task_id = task_id.to_string();
        let mut loading = false;
        if let Some(dl) = state.downloads.get_mut(&task_id) {
            dl.delete_when_done = true;
            loading = dl.loading;
        }
        crate::cancel::network::on_fs_download(&task_id);
        if !loading {
            finish(state, &task_id);
        }
        return;
    }
    delete_entry(state, &req.name);
}

pub fn on_delete_result(state: &mut State, response: veldsdk::proto::fs::FsDeleteResult) {
    let Some(path) = state.pending_delete.take(&veldsdk::correlation()) else { return };
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "failed to delete {}: {}", path, response.error);
    }
    catalog::rescan(state);
}

pub fn on_fs_download_progress(state: &mut State, event: FsDownloadProgress) {
    let Some(dl) = state.downloads.get_mut(&veldsdk::correlation()) else { return };
    dl.done = event.downloaded_bytes;
    dl.total = event.total_bytes;

    if event.total_bytes > 0 {
        let name = dl.name.clone();
        let known = state.origins.get(&name).cloned()
            .filter(|origin| origin.total_bytes != Some(event.total_bytes));
        if let Some(known) = known {
            write_sidecar(state, &name, storage::OriginSidecar {
                total_bytes: Some(event.total_bytes),
                ..known
            });
        }
    }
    catalog::publish(state);
}

pub fn on_fs_download_result(state: &mut State, response: FsDownloadResponse) {
    let correlation_id = veldsdk::correlation();
    if !response.error.is_empty() {
        let name = state.downloads.get(&correlation_id).map(|d| d.name.clone());
        if let Some(name) = name {
            veldsdk::log::warn!(target: "handlers", "download {} failed: {}", name, response.error);
            state.troubles.insert(name, response.error.clone());
        }
    }
    finish(state, &correlation_id);
}

fn finish(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.remove(id) else { return };

    start_waiting(state);

    if dl.delete_when_done {
        delete_entry(state, &dl.name);
        return;
    }

    catalog::rescan(state);
}
