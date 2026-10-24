//! Opening a downloaded file as a resource for the caller.
//!
//! The library opens only what it has. A product that is not in the catalogue
//! is opened by the provider (data-provider/on_open): only it can sign a
//! request to the storage, and the library knows nothing about signing exactly
//! as the provider knows nothing about the disk layout.

use crate::module::{ReadPurpose, State};
use crate::proto::data_library::{ItemRequest, OpenRequest};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;
use veldsdk::resource;

/// An open waiting for the catalogue traversal to finish.
///
/// Waiting requires the owner and reply address to be saved: both are read
/// from the call context, and by the time of resumption the context is already
/// someone else's.
pub struct Deferred {
    pub name: String,
    pub owner: String,
    pub reply_to: String,
}

/// Retries the deferred opens — called when the catalogue traversal has
/// finished.
pub fn resume(state: &mut State) {
    for waiting in std::mem::take(&mut state.deferred_opens) {
        match state.entry_for(&waiting.name) {
            Some(entry) if entry.is_partial => {
                fail(waiting.reply_to, format!("'{}' is not fully downloaded", waiting.name));
            }
            Some(_) => read_file(state, waiting.name, waiting.owner, waiting.reply_to),
            None => {
                fail(waiting.reply_to, format!("no entry '{}' in the catalogue", waiting.name));
            }
        }
    }
}

pub fn on_open(state: &mut State, req: OpenRequest) {
    let reply_to = veldsdk::correlation();
    let owner = match resource::requester("data-library/on_open") {
        Ok(owner) => owner,
        Err(e) => return fail(reply_to, e),
    };
    let Some(entry) = state.entry_for(&req.name) else {
        // The catalogue is not read yet — this is not "no such entry" but "do
        // not know yet": the disk traversal runs as its own job, and the click
        // on "view" beats it in the first seconds of startup. Answering with a
        // refusal here would kill the viewing tab for our own slowness — and
        // there is no one to retry: the caller asks once.
        if !state.pending_list.is_empty() {
            veldsdk::log::debug!(target: "handlers",
                "open of '{}' waits for the first catalogue traversal", req.name);
            state.deferred_opens.push(Deferred { name: req.name, owner, reply_to });
            return;
        }
        return fail(reply_to, format!("no entry '{}' in the catalogue", req.name));
    };
    if entry.is_partial {
        return fail(reply_to, format!("'{}' is not fully downloaded", req.name));
    }
    read_file(state, req.name, owner, reply_to);
}

/// Asks the file system for the bytes of the entry and remembers whom to hand
/// them to.
fn read_file(state: &mut State, name: String, owner: String, reply_to: String) {
    // Partial downloads are cut off above, so the file lies under its own name.
    let path = crate::module::storage::file_path(&name);
    // The external id goes back in the answer; our own is the wait key, and by
    // it the answer is recognized as a "file open" rather than a sidecar read.
    let correlation_id = state.pending_reads.begin(ReadPurpose::File(OpenFor {
        owner,
        reply_to,
    }));
    crate::calls::fs::on_read(&FsReadRequest { path }, &correlation_id);
}

/// Who receives the opened resource and which request it answers.
pub struct OpenFor {
    pub owner: String,
    pub reply_to: String,
}

/// fs opened the file we requested for the caller (`target` is the settled
/// wait, see module::on_read_result). Ownership goes to the caller: from now
/// on it reads the resource as it likes and decides itself when to close.
pub fn on_file_opened(target: OpenFor, opened: &ResourceOpened) {
    crate::emit::on_open_result(&resource::relay(opened, &target.owner), &target.reply_to);
}

fn fail(correlation_id: String, error: String) {
    crate::emit::on_open_result(&resource::opened(Err(error)), &correlation_id);
}

/// Show the entry in the file manager. The path is computed by us: the storage
/// layout is ours and does not leak out, while the showing is done by the
/// platform, because only it has a desktop.
///
/// A partial one is shown under its `.part`: that is what actually lies on
/// disk, and opening an empty place instead of it would be a lie.
pub fn on_reveal(state: &mut State, req: ItemRequest) {
    if req.name.is_empty() { return; }
    crate::calls::app::on_reveal_path(&veldsdk::proto::app::RevealPath {
        path: crate::module::storage::data_path(&req.name, state.is_partial(&req.name)),
    });
}
