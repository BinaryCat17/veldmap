//! Cache layout — the one place where it is known.
//!
//! A directory per source, a file per tile: fs writes atomically and whole, so
//! a pack file would be rewritten entirely at every fill, while tile files are
//! appended and lost independently — a lost one means a miss, not a breakage.
//! meta.json is a tiny directory marker: its mtime is "when the source was
//! last used", and eviction picks its victims by it.

/// Cache root. In .gitignore together with all of runtime/data.
pub const ROOT: &str = "data/tiles";

/// Marker name in a source directory.
pub const META: &str = "meta.json";

/// Marker contents. Constant: rewriting the same text moves the mtime —
/// exactly what it is rewritten for on every use.
pub const META_BODY: &[u8] = b"{\"veldmap\":\"tile-cache\"}\n";

/// Sanity ceiling on the tile side. The read path asks for it, where the side
/// becomes known; the write path measures against a byte ceiling derived from
/// it. The tile side is a producer constant, but the cache survives its
/// change, and a texture must be allocated for the tile regardless — so the
/// boundary must be its own.
pub const MAX_TILE_SIDE: u32 = 4096;

/// Ceiling on the tile body — the same side, only in RGBA bytes. Derived, not
/// spelled out as a number: diverging from the side, it would silently reject
/// legitimate tiles, the cache for such a source would never fill, and every
/// display would go through a full pass over the file.
pub const MAX_TILE_BYTES: usize = (MAX_TILE_SIDE as usize).pow(2) * 4;

pub fn source_dir(key: &str) -> String {
    format!("{}/{}", ROOT, key)
}

pub fn meta_path(key: &str) -> String {
    format!("{}/{}/{}", ROOT, key, META)
}

pub fn tile_path(key: &str, level: u32, x: u32, y: u32) -> String {
    format!("{}/{}/{}_{}_{}.qoi", ROOT, key, level, x, y)
}

/// The key arrives in a message and ends up in a path, so its alphabet is
/// closed: lowercase hex digits, Latin letters, and a hyphen — exactly what
/// the tiler's fingerprint produces. Anything else is not a key but an
/// attempt to walk someone else's path.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
