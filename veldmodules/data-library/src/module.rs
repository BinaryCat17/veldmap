pub mod storage;
pub mod catalog;
pub mod download;
pub mod open;

use std::collections::HashMap;
use storage::{LocalFile, OriginSidecar};

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    pub snapshot: HashMap<String, LocalFile>,
    pub origins: HashMap<String, OriginSidecar>,
    pub downloads: HashMap<String, Download>,
    pub queue_tick: u64,
    pub troubles: HashMap<String, String>,

    pub pending_list: veldsdk::Correlator<()>,
    pub list_again: bool,
    pub pending_roots: veldsdk::Latest,
    pub pending_reads: veldsdk::Correlator<ReadPurpose>,
    pub pending_sidecar_writes: veldsdk::Correlator<SidecarWrite>,
    pub queued_sidecars: HashMap<String, OriginSidecar>,
    pub deferred_opens: Vec<crate::module::open::Deferred>,
    pub pending_delete: veldsdk::Correlator<String>,
}

pub enum ReadPurpose {
    Sidecar(String),
    File(open::OpenFor),
}

pub struct Download {
    pub identifier: String,
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub delete_when_done: bool,
    pub queued: u64,
    pub waiting: bool,
    pub loading: bool,
}

pub struct SidecarWrite {
    pub region: u64,
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
    pub fn entry_for(&self, name: &str) -> Option<&LocalFile> {
        self.snapshot.get(name)
    }

    pub fn forget_gone_troubles(&mut self) {
        let (origins, snapshot) = (&self.origins, &self.snapshot);
        self.troubles.retain(|name, _| origins.contains_key(name) || snapshot.contains_key(name));
    }

    pub fn is_partial(&self, name: &str) -> bool {
        self.entry_for(name).map_or(true, |file| file.is_partial)
    }

    pub fn active_download(&self, name: &str) -> Option<(&str, &Download)> {
        self.downloads.iter()
            .find(|(_, d)| d.name == name)
            .map(|(id, d)| (id.as_str(), d))
    }

    pub fn identifier_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.identifier.as_str())
    }

    pub fn product_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.product.as_str())
    }

    pub fn siblings_of(&self, name: &str) -> u32 {
        self.origins.get(name).map(|o| o.siblings).unwrap_or(0)
    }

    pub fn total_bytes(&self, name: &str) -> u64 {
        self.origins.get(name).and_then(|o| o.total_bytes).unwrap_or(0)
    }
}

pub use catalog::{on_list, on_list_result, on_product_roots_result, on_snapshot, on_write_result};

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
