//! data-library: the catalog of downloaded files.
//!
//! Owns all local storage — the catalog, the `.part` suffix, the `.origin`
//! sidecars — and exposes only derived state (see types.proto). No consumer
//! knows how this lies on disk.
//!
//! The state is derived, not accumulated. Three independent sources of facts:
//! the disk snapshot (fs/on_list), the sidecars (where things came from) and
//! the running downloads. None is patched "optimistically": the snapshot is
//! reread on every terminal event. Storing rows materialized would mean editing
//! them from four places, and every missed patch would lie to the UI until the
//! next listing.
//!
//! module.rs is the facade: State, init, and re-exports of the handlers. The
//! logic lives in catalog.rs (the snapshot and sidecars), download.rs (the
//! downloads) and open.rs.

pub mod storage;
pub mod catalog;
pub mod download;
pub mod open;

use std::collections::HashMap;
use storage::{LocalFile, OriginSidecar};

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    /// What lies on disk: entry name → what was found under it. The single
    /// writer is catalog::on_list_result.
    ///
    /// The key is the entry name, not the file name: `foo` and `foo.part` are
    /// one catalog entry in two states, and keeping them as two rows would
    /// create two answers to the question "what is with this entry", selected
    /// by the catalog walk order.
    pub snapshot: HashMap<String, LocalFile>,
    /// Sidecar contents by entry name. A disk cache, not a separate truth: the
    /// listing trims it to what actually lies in the catalog, so a file deleted
    /// outside the application will not resurrect as a record of intent.
    pub origins: HashMap<String, OriginSidecar>,
    /// Running downloads — a domain registry, not a correlation table: an entry
    /// has progress, two possible terminal events and its own place in the
    /// derived state. The key is the operation id, also the task_id of the task
    /// at the platform: we name the operation because we are also its owner
    /// (see download.rs). The entry lives from the "download" press to the
    /// terminal event — including the signature-waiting window, when the task
    /// does not exist yet.
    pub downloads: HashMap<String, Download>,
    /// The next number in the download queue. It grows and is never reused: the
    /// number is only needed to compare two entries with each other.
    pub queue_tick: u64,
    /// Why the last attempt failed, by entry name. It lives here, not in the
    /// sidecar: the reason concerns the attempt, not the file, and it has no
    /// need to survive a restart. It does survive a catalog rescan: the disk
    /// walk knows nothing about failures and would silently erase them all.
    pub troubles: HashMap<String, String>,

    /// Waiting for fs/on_list — cancels a stale answer. There is always
    /// exactly one request here: disk walks happen only as many as needed for
    /// the state to be correct, not as many as there were reasons to rescan
    /// (see `catalog::rescan`).
    pub pending_list: veldsdk::Correlator<()>,
    /// A reason to rescan that arrived while a walk was running. One flag, not
    /// a counter: no matter how many accumulate, one walk catches up with all
    /// of them — it sees the whole disk.
    pub list_again: bool,
    /// A question to the provider: "which scenes do these keys belong to".
    /// Only the last one is current: sidecars are read one by one, and each one
    /// read makes the question more complete — there is no point in answering
    /// from a stale list.
    pub pending_roots: veldsdk::Latest,
    /// fs reads handed out and not yet answered. One table per topic, not a
    /// table per purpose (see [`veldsdk::Correlator`]): the purpose is
    /// distinguished by `ReadPurpose`.
    pub pending_reads: veldsdk::Correlator<ReadPurpose>,
    /// Waiting for fs/on_write of a sidecar.
    pub pending_sidecar_writes: veldsdk::Correlator<SidecarWrite>,
    /// Sidecars waiting for the previous one of the same entry to finish
    /// writing. By name, not as a queue: a sidecar is written whole, and
    /// intermediate states are of no use to the disk — it is always the latest
    /// one that waits (see `catalog::write_sidecar`).
    pub queued_sidecars: HashMap<String, OriginSidecar>,
    /// Opens that arrived before the first catalog walk: answering them "no
    /// such entry" would refuse them for our not having read the disk in time
    /// (see `open::resume`).
    pub deferred_opens: Vec<crate::module::open::Deferred>,
    /// Waiting for fs/on_delete — the context is the path of the deleted file.
    pub pending_delete: veldsdk::Correlator<String>,
}

/// Why the library reads a file. Both reads go over one fs/read topic and
/// return one `core.ResourceOpened` — only this distinguishes them.
pub enum ReadPurpose {
    /// A sidecar of an entry; the context is its name.
    Sidecar(String),
    /// A file opened for the caller: who receives the ownership and which
    /// request we answer.
    File(open::OpenFor),
}

/// One running download — the only source of byte progress while it is alive.
/// After it, progress is taken from the disk (the `.part` size in the snapshot).
pub struct Download {
    pub identifier: String,
    pub name: String,
    pub done: u64,
    /// 0 if the server did not send Content-Length.
    pub total: u64,
    /// The trash was pressed during the download. Deleting a `.part` over an
    /// active entry is not allowed (the host holds the file open), so we kill
    /// the download first, and the delete fires on its terminal answer. The
    /// flag is here, not in a separate table: its key would be the same and it
    /// would be read on the same topic — that is, one topic would carry two
    /// tables, exactly what the rule at `pending_reads` protects against.
    pub delete_when_done: bool,
    /// Place in the queue: the smaller, the earlier the press. The start order
    /// is taken from here, not from walking the registry — it is a hash table,
    /// and the same selection would go downloading in one order one time and
    /// another the next.
    pub queued: u64,
    /// Waiting for a slot: the entry is created but the signature has not been
    /// requested yet. It differs from "waiting for the signature" in exactly
    /// this — it will not move on its own, a freed slot starts it (see
    /// `download::AT_ONCE`).
    pub waiting: bool,
    /// The request has gone to the network. Before that there is no download
    /// at the platform at all — no task, no terminal event — and taking the
    /// entry off the books is up to us; after that the terminal answer does it
    /// for us, whatever it is.
    ///
    /// This is how the two kinds of "nothing to kill" differ: the download is
    /// still waiting for the signature — or it has already finished on its own
    /// and its answer is on the way. Removing the entry in the second case
    /// loses the failure reason: the arriving answer has nowhere to attribute
    /// it.
    pub loading: bool,
}
// The share (`progress` from DownloadProgress) is intentionally absent here: it
// is fully derived from done/total, and storing it alongside would mean keeping
// two numbers that can diverge.

/// A sidecar handed to fs/on_write and not yet confirmed.
pub struct SidecarWrite {
    /// CPU region holding the sidecar body — released on the host's answer.
    pub region: u64,
    /// Entry name: while the write is in flight the sidecar may not be on disk
    /// yet, and the `origins` trim by the listing must not treat it as deleted.
    pub name: String,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    let state = State {
        snapshot: HashMap::new(),
        origins: HashMap::new(),
        queued_sidecars: HashMap::new(),
        deferred_opens: Vec::new(),
        downloads: HashMap::new(),
        queue_tick: 0,
        troubles: HashMap::new(),
        pending_list: veldsdk::Correlator::new(),
        list_again: false,
        pending_roots: veldsdk::Latest::new(),
        pending_reads: veldsdk::Correlator::new(),
        pending_sidecar_writes: veldsdk::Correlator::new(),
        pending_delete: veldsdk::Correlator::new(),
    };
    Ok(state)
}

impl State {
    /// The on-disk entry with the given name (complete or partially
    /// downloaded).
    pub fn entry_for(&self, name: &str) -> Option<&LocalFile> {
        self.snapshot.get(name)
    }

    /// Forgets failure reasons of entries that no longer exist.
    ///
    /// The reason belongs to the entry, not the name: having outlived the entry
    /// itself, it would one day be attributed to a new one under the same name
    /// — one that failed not in this run.
    pub fn forget_gone_troubles(&mut self) {
        let (origins, snapshot) = (&self.origins, &self.snapshot);
        self.troubles.retain(|name, _| origins.contains_key(name) || snapshot.contains_key(name));
    }

    /// Whether the entry's bytes lie under `.part`. One answer for all askers:
    /// deletion and the file-manager reveal ask the same thing, and two
    /// defaults for one question would one day diverge.
    ///
    /// What is missing from the snapshot entirely counts as `.part`: the only
    /// thing that could remain from a download that failed between disk walks.
    pub fn is_partial(&self, name: &str) -> bool {
        self.entry_for(name).map_or(true, |file| file.is_partial)
    }

    /// The entry's running download along with its task_id (needed for
    /// cancellation).
    pub fn active_download(&self, name: &str) -> Option<(&str, &Download)> {
        self.downloads.iter()
            .find(|(_, d)| d.name == name)
            .map(|(id, d)| (id.as_str(), d))
    }

    /// The provider key of the entry, if the sidecar has been read.
    pub fn identifier_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.identifier.as_str())
    }

    /// The scene the entry belongs to. Only the sidecar knows this: the on-disk
    /// name says nothing about the scene.
    pub fn product_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.product.as_str())
    }

    /// How many files are in this entry's scene; 0 — the scene has not been
    /// walked in full, and nothing can be said about its completeness.
    pub fn siblings_of(&self, name: &str) -> u32 {
        self.origins.get(name).map(|o| o.siblings).unwrap_or(0)
    }

    /// The expected full size from the sidecar; 0 — Content-Length has not
    /// been seen yet.
    pub fn total_bytes(&self, name: &str) -> u64 {
        self.origins.get(name).and_then(|o| o.total_bytes).unwrap_or(0)
    }
}

// -- Input handlers --
pub use catalog::{on_list, on_list_result, on_product_roots_result, on_snapshot, on_write_result};

/// fs/on_read_result — one topic, two purposes: the catalog finishes reading
/// sidecars, open hands the file to the caller. The fork is here, not in the
/// schema, because what distinguishes them is not the bus contract but the
/// purpose recorded at the request.
pub fn on_read_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    match state.pending_reads.take(&veldsdk::correlation()) {
        Some(ReadPurpose::Sidecar(name)) => catalog::on_sidecar_read(state, name, &opened),
        Some(ReadPurpose::File(target)) => open::on_file_opened(target, &opened),
        None => veldsdk::resource::discard("fs/on_read_result", opened),
    }
}
pub use download::{on_download, on_pause, on_delete, on_delete_result,
                   on_signed, on_fs_download_progress, on_fs_download_result};
pub use open::{on_open, on_reveal};
