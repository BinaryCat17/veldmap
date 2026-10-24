//! Cache filling and file writes.
//!
//! Writes are fire-and-forget: the body travels to fs as a CPU resource,
//! ownership lives in the wait table until confirmation, and a failure is a
//! log line and a future miss, not a breakage. Atomicity is on the fs side
//! (tmp + rename): there is never a half-tile on disk.

use veldsdk::proto::fs::{FsWriteRequest, FsWriteResult};

use crate::module::{evict, layout, State};
use crate::proto::tile_cache::StoreTile;

/// CPU region sent to fs/on_write and not yet confirmed.
pub struct PendingWrite {
    /// Released when the reply arrives.
    pub region: u64,
    /// Where the write went — for failure logging.
    pub path: String,
}

pub fn on_store(state: &mut State, tile: StoreTile) {
    if !layout::valid_key(&tile.fingerprint) {
        veldsdk::log::warn!(target: "handlers", "store with invalid key: '{}'", tile.fingerprint);
        return;
    }
    // What lands on disk is checked — the bytes. Sizes are neither available
    // nor needed here: the codec carries them, and the read path parses them
    // (`query::decode`), where they are held to their own plausibility.
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

/// Marks a source alive: rewrites the directory marker, moving its mtime.
///
/// Not on every tile: freshness is measured in days, while the marker is a
/// disk write, and paying it for every cell would mean writing twice as much
/// as is shown. But not once per session either: the list of marked sources
/// is cleared by every eviction sweep (see `evict::maybe_sweep`), so the
/// marker of an active source is guaranteed to be no older than one sweep
/// window. Otherwise, in a long session it would date from the start, and the
/// very source being looked at right now would become the victim.
pub fn touch(state: &mut State, key: &str) {
    if state.touched.insert(key.to_string()) {
        write_file(state, layout::meta_path(key), layout::META_BODY);
    }
}

/// Puts bytes into a file through fs: region → on_write → release on reply
/// (see PendingWrite).
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
        // Ownership travels to the wait table: on_write_result releases it.
        handle: Some(region.into_handle()),
    }, &correlation);
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(write) = state.pending_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !result.error.is_empty() {
        // A lost write is a future miss: the producer rewrites it.
        veldsdk::log::warn!(target: "handlers", "{} not written: {}", write.path, result.error);
    }
}
