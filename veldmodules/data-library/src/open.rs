use crate::module::{ReadPurpose, State};
use crate::proto::data_library::{ItemRequest, OpenRequest};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;
use veldsdk::resource;

pub struct Deferred {
    pub name: String,
    pub owner: String,
    pub reply_to: String,
}

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

fn read_file(state: &mut State, name: String, owner: String, reply_to: String) {
    let path = crate::module::storage::file_path(&name);
    let correlation_id = state.pending_reads.begin(ReadPurpose::File(OpenFor {
        owner,
        reply_to,
    }));
    crate::calls::fs::on_read(&FsReadRequest { path }, &correlation_id);
}

pub struct OpenFor {
    pub owner: String,
    pub reply_to: String,
}

pub fn on_file_opened(target: OpenFor, opened: &ResourceOpened) {
    crate::emit::on_open_result(&resource::relay(opened, &target.owner), &target.reply_to);
}

fn fail(correlation_id: String, error: String) {
    crate::emit::on_open_result(&resource::opened(Err(error)), &correlation_id);
}

pub fn on_reveal(state: &mut State, req: ItemRequest) {
    if req.name.is_empty() { return; }
    crate::calls::app::on_reveal_path(&veldsdk::proto::app::RevealPath {
        path: crate::module::storage::data_path(&req.name, state.is_partial(&req.name)),
    });
}
