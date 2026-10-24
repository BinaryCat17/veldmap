//! What a tile is made of and how many are asked for at once — the one thing
//! the cache, the producer, and the consumers must all agree on.
//!
//! The file is included three times: by the module itself
//! (`crate::module::tile`), by its wrap crate (`wraps/rust/src/wrap.rs` via
//! `#[path]`), and by the tiler's wrap crate
//! (`image-tiler/wraps/rust/src/wrap.rs`, where the consumers size their
//! appetite) — because a second copy of the constants would eventually drift
//! apart from the first. Anything from `crate::` would silently break
//! inclusion in a wrap; only a library declared to all crates qualifies
//! (`rust.wrap_dependencies` in config.yaml), and for now the file needs
//! nothing beyond `veldsdk`, which every crate has by construction.
//!
//! The on-disk cache layout does not belong here and stays with the cache
//! (`layout.rs`): a consumer has no business there.

/// Tile texture format. sRGB, because a tile holds display-ready content: the
/// consumer's sampler yields linear values, and filtering at fractional scales
/// passes through linear space.
///
/// The two must not diverge: tiles from the cache and from the producer land
/// in one frame, and a different format would give a different brightness
/// there — a checkerboard that changes the picture from run to run as the
/// cache fills.
pub const TILE_FORMAT: veldsdk::graphics::TextureFormat =
    veldsdk::graphics::TextureFormat::TexRgba8UnormSrgb;

/// Ceiling on the number of tiles in a single cache query. A screen is tens of
/// tiles; thousands mean a mistake in the client's visibility math, and
/// refusing is more honest than silently opening thousands of files.
///
/// The client cuts its request down to the video memory budget
/// (`tiles::Store::cap_tiles`, 128 cells at the default 256 MiB) and caps it
/// with this same number — so it never sends a list the cache would refuse,
/// and it has no way out of such a refusal: it would just send the same list
/// again.
pub const MAX_QUERY_TILES: usize = 1024;
