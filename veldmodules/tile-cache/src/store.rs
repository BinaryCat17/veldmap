use veldsdk::proto::fs::{FsWriteRequest, FsWriteResult};

use crate::module::{evict, layout, State};
use crate::proto::tile_cache::StoreTile;

pub struct PendingWrite {
    pub region: u64,
    pub path: String,
}

pub fn on_store(state: &mut State, tile: StoreTile) {
    if !layout::valid_key(&tile.fingerprint) {
        veldsdk::log::warn!(target: "handlers", "store with invalid key: '{}'", tile.fingerprint);
        return;
    }
    if tile.qoi.is_empty() || tile.qoi.len() > layout::MAX_TILE_BYTES {
        veldsdk::log::warn!(target: "handlers",
            "store {}:{}:{}:{} rejected: {} bytes",
            tile.fingerprint, tile.level, tile.x, tile.y, tile.qoi.len());
        return;
    }

    touch(state, &tile.fingerprint);
    write_file(state, layout::tile_path(&tile.fingerprint, tile.level, tile.x, tile.y), &tile.qoi);

    state.stores_since_sweep += 1;
    evict::maybe_sweep(state);
}

/// Marks a source alive: rewrites its directory marker, once per sweep window —
/// the touched set is cleared by every eviction sweep, so an active source's
/// marker is never older than a window and is never the sweep's victim.
pub fn touch(state: &mut State, key: &str) {
    if state.touched.insert(key.to_string()) {
        write_file(state, layout::meta_path(key), layout::META_BODY);
    }
}

pub fn write_file(state: &mut State, path: String, data: &[u8]) {
    let region = match veldsdk::resource::region_of(data) {
        Ok(region) => region,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{} not written: {}", path, error);
            return;
        }
    };
    let correlation =
        state.pending_writes.begin(PendingWrite { region: region.id(), path: path.clone() });
    crate::calls::fs::on_write(&FsWriteRequest {
        path,
        handle: Some(region.into_handle()),
    }, &correlation);
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(write) = state.pending_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !result.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "{} not written: {}", write.path, result.error);
    }
}
