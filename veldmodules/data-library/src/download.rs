//! Product acquisition: signing at the provider, download, pause, removal.
//!
//! An operation has one name along the whole path: our `correlation_id` — also
//! the request id to the provider, the request id to network, and the operation
//! name at the platform. The platform makes the publisher of
//! `network/on_fs_download` the owner — that is us — so we kill it ourselves.
//!
//! The provider takes part in exactly one step — it signs the address. The
//! storage layout never leaves the library: we substitute the path ourselves,
//! already after the signing.

use crate::module::{Download, State};
use crate::module::storage;
use crate::module::catalog::{self, delete_data, delete_entry, write_sidecar};
use crate::proto::data_library::{DownloadRequest, ItemRequest};
use crate::proto::data_provider::{SignRequest, SignedUrl};
use veldsdk::proto::network::{FsDownloadProgress, FsDownloadRequest, FsDownloadResponse};

/// How many downloads run at once. The rest wait for a slot and start as soon
/// as it is freed.
///
/// The cap is needed not by us but by the other side: a batch "download" on a
/// page of twenty scenes is hundreds of files with one press, and the storage
/// answers such requests with refusals, not data. Ten sits with margin below
/// where the storage starts refusing; the exact value is not measured, and
/// backing it with a number would be a fabrication.
///
/// A waiting entry is visible in the list on equal terms with running ones —
/// with whatever it already has on disk, or with zero for a re-download: from
/// the "download" press it is already the user's intent, and hiding it before
/// the start would lose the press from view. The signature-waiting window looks
/// the same — it is the same pause, only shorter (see [`Download::waiting`]).
const AT_ONCE: usize = 10;

/// Download or resume a product. Where it lands is our decision: the storage
/// layout is ours, and the caller has no need to know it.
pub fn on_download(state: &mut State, req: DownloadRequest) {
    if req.identifier.is_empty() { return; }

    // When the scene is not named, it is taken from what is already recorded:
    // a re-download of the same file arrives without it, and forgetting which
    // scene a file belongs to would remove it from there forever — we cannot
    // ask for this boundary again, the provider knows it. Search by key, not
    // by name: the name comes from the scene and is not fetched yet.
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

    // A repeated press while the previous attempt is running (including while
    // it waits for the signature). Cut off here: the operation id is ours from
    // the very start, so there is no window where a download runs but is not
    // yet accounted for.
    if state.downloads.values().any(|d| d.name == name) {
        veldsdk::log::info!(target: "handlers", "{} is already downloading, ignoring the repeat", name);
        return;
    }

    // The reason of the past failure concerns the past attempt, and a new one
    // clears it: while it runs there is nothing to say about it, and if it
    // fails it will tell its own.
    state.troubles.remove(&name);

    // The sidecar goes in at once, before the signing: even if the application
    // dies at the first byte, the disk will already know where the file comes
    // from and the record of intent will not vanish. From the previous sidecar
    // we carry over what is known about the entry but was not obtained by this
    // request: the expected size from the past attempt and the scene
    // composition, if it has been walked.
    let (total_bytes, siblings) = state.origins.get(&name)
        .map_or((None, 0), |origin| (origin.total_bytes, origin.siblings));
    write_sidecar(state, &name, storage::OriginSidecar {
        provider: storage::PROVIDER_NAME.to_string(),
        identifier: req.identifier.clone(),
        total_bytes,
        product: product.clone(),
        siblings,
    });

    // Seed with what is already known, not with zeros: for a partially
    // downloaded entry the download continues from the bytes already on disk,
    // and "0 B" at the resume would be a lie. A completed entry has no `.part`
    // at all — a re-download starts from zero, and the `is_partial` filter
    // says exactly this.
    let done = state.entry_for(&name).filter(|file| file.is_partial).map(|file| file.size).unwrap_or(0);
    let total = state.total_bytes(&name);

    // We name the operation: with this same id we ask for the signature,
    // request the download and cancel the task at the platform — it is ours
    // from start to finish.
    let correlation_id = veldsdk::generate_id();
    let queued = state.queue_tick;
    state.queue_tick += 1;
    // No free slot — the entry joins the queue, not the network. It starts by
    // itself when one of the running ones finishes (see [`start_waiting`]).
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

/// Put an entry into motion: clear the waiting flag and ask for the signature.
///
/// The only place where a download starts — and therefore "waiting" and
/// "un-signed" are one and the same here, not two facts kept in agreement. A
/// second start bypassing this function would silently split them: an entry
/// would count as waiting while already downloading, and the queue would start
/// it a second time under the same id.
///
/// Re-downloading a completed file deletes it here, not at the press. The
/// downloader writes to `.part` and renames it only at the end, so otherwise a
/// second, partially downloaded file would lie next to the finished one under
/// the same entry name — while "one name, one entry" holds exactly because such
/// a pair never exists on disk. But a queue stands between the press and the
/// start, and a file deleted in advance would leave the user without data and
/// without a download for the whole waiting time.
///
/// The slot is held until a terminal event, and a signature that never answers
/// has none: there is nothing with which to kill the request to the provider,
/// and such an entry stands in the queue until a pause removes it. This is a
/// live limitation — a signature has no deadline of its own.
fn start(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.get_mut(id) else { return };
    dl.waiting = false;
    let (identifier, name) = (dl.identifier.clone(), dl.name.clone());

    if state.entry_for(&name).is_some_and(|file| !file.is_partial) {
        delete_data(state, &name);
    }
    crate::calls::data_provider::on_sign(&SignRequest { identifier }, id);
}

/// How many downloads are already running: slot-waiting entries do not count,
/// everything else runs — including those waiting for a signature, their
/// request has already started.
fn busy(state: &State) -> usize {
    state.downloads.values().filter(|dl| !dl.waiting).count()
}

/// Start the next one in the queue if a slot has been freed.
///
/// One per freed slot, not "as many as fit": this is called from every end of
/// a download, and there are exactly as many slots as ends.
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

/// The address is signed — we download. The on-disk path is substituted here:
/// the provider does not know the storage layout and must not.
pub fn on_signed(state: &mut State, signed: SignedUrl) {
    let correlation_id = veldsdk::correlation();
    let Some(dl) = state.downloads.get(&correlation_id) else { return };
    let name = dl.name.clone();

    // An empty answer with an empty reason is not success but an answer the
    // host supplied on behalf of the worker: taken for a signature, it would
    // send the download into the void and tell the user about the network
    // where the problem is a crashed module.
    let refusal = veldsdk::reply::undelivered(&signed.error)
        .or_else(|| (!signed.error.is_empty()).then(|| signed.error.clone()));
    if let Some(why) = refusal {
        veldsdk::log::warn!(target: "handlers", "signature for {} failed: {}", name, why);
        state.troubles.insert(name, format!("signature failed: {}", why));
        // There is no task yet — no platform terminal event will come, so we
        // take the entry off the books ourselves, otherwise it would stay
        // "downloading" forever.
        finish(state, &correlation_id);
        return;
    }

    crate::calls::network::on_fs_download(&FsDownloadRequest {
        url: signed.url,
        path: storage::file_path(&name),
        headers: signed.headers,
    }, &correlation_id);
    // From this moment the platform, not us, declares the end of the download.
    if let Some(dl) = state.downloads.get_mut(&correlation_id) {
        dl.loading = true;
    }
}

/// Pause: the `.part` stays on disk exactly where the break left it, and the
/// next "download" continues from the interrupted byte. We kill it ourselves:
/// the operation is ours. There is nothing to clean up here — the terminal
/// `on_fs_download_result` will arrive anyway, the host publishes it on behalf
/// of the killed downloader.
///
/// Giving up on a started download belongs not here but in `on_delete`:
/// stopping with keeping and stopping with discarding are different things, and
/// there are two topics for them, not one with two meanings.
pub fn on_pause(state: &mut State, req: ItemRequest) {
    let Some((task_id, dl)) = state.active_download(&req.name) else { return };
    let (task_id, loading) = (task_id.to_string(), dl.loading);
    crate::cancel::network::on_fs_download(&task_id);
    // We take the entry off the books ourselves only before the request has
    // gone to the network: there is no task at the platform yet, no terminal
    // event will come, and an arriving signature would start the "paused"
    // download.
    //
    // After that we never remove it, even when there was nothing to kill: that
    // means the download finished on its own and its answer is already on the
    // way. Removing the entry here would take away its name, and the failure
    // reason would vanish with it.
    if !loading {
        finish(state, &task_id);
    }
}

/// Delete an entry — complete, partially downloaded, or declared by a sidecar
/// alone. This also cancels a running download: giving up on a started download
/// is expressed by the same topic, because it must leave behind the same thing
/// — nothing.
pub fn on_delete(state: &mut State, req: ItemRequest) {
    // The file is being downloaded right now — the host holds the `.part` open
    // for writing, and deleting over an active entry is not allowed. We kill
    // the download and mark the entry for deletion: `abort` does not wait for
    // the downloader to release the file, so the delete itself happens on the
    // terminal answer — by then the future is already dropped together with the
    // descriptor.
    if let Some((task_id, _)) = state.active_download(&req.name) {
        let task_id = task_id.to_string();
        let mut loading = false;
        if let Some(dl) = state.downloads.get_mut(&task_id) {
            dl.delete_when_done = true;
            loading = dl.loading;
        }
        crate::cancel::network::on_fs_download(&task_id);
        // Before the network we declare the end ourselves (see on_pause); the
        // deferred delete fires right here — finish reads the flag.
        if !loading {
            finish(state, &task_id);
        }
        return;
    }
    delete_entry(state, &req.name);
}

/// Broadcast topic — we match the correlation_id so as not to accept someone
/// else's answer.
pub fn on_delete_result(state: &mut State, response: veldsdk::proto::fs::FsDeleteResult) {
    let Some(path) = state.pending_delete.take(&veldsdk::correlation()) else { return };
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "failed to delete {}: {}", path, response.error);
    }
    // We rescan in any case: on error — to show what is actually on disk, on
    // success — so the entry goes away.
    catalog::rescan(state);
}

pub fn on_fs_download_progress(state: &mut State, event: FsDownloadProgress) {
    let Some(dl) = state.downloads.get_mut(&veldsdk::correlation()) else { return };
    dl.done = event.downloaded_bytes;
    dl.total = event.total_bytes;

    // We write Content-Length to the sidecar when a new value is learned (not
    // on every event): otherwise after a restart "out of how much" is unknown
    // again until a new download starts — the sidecar is the only thing that
    // survives a restart.
    if event.total_bytes > 0 {
        let name = dl.name.clone();
        // We fix one field on top of the written sidecar: everything else
        // arrived with the download request, and here these facts are nowhere
        // to take from — rewriting them with empty values would lose both the
        // origin and the scene.
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

/// The single end of a download, whatever it is: success, network error, or a
/// kill.
///
/// A non-empty error is exactly a failure: a killed download has no domain
/// result, the host publishes an empty answer for it. This is how a failed
/// download differs from one stopped by the user — nothing else distinguishes
/// them.
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

/// Takes the download off the books and brings the catalog in line with the
/// disk.
///
/// Intentionally idempotent: an early refusal (the signature failed) happens
/// before the operation is even registered, and no terminal answer will come
/// for it — this path leads here directly.
fn finish(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.remove(id) else { return };

    // A slot is freed — start the next one. Before analyzing how this one
    // ended: the queue must wait neither for a catalog rescan nor for a
    // deletion.
    start_waiting(state);

    // The trash was pressed during the download — then this is not a
    // cancellation for its own sake but a deferred deletion: the `.part` stayed
    // exactly where abort left it, and now it can be safely deleted.
    if dl.delete_when_done {
        delete_entry(state, &dl.name);
        return;
    }

    // We rescan the catalog: the size of the `.part`/finished file is taken
    // from the disk, not carried by hand from the registry into the entry.
    catalog::rescan(state);
}
