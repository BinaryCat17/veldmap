//! Serving a query: named tiles from cache files into textures.
//!
//! Opening a file is an fs event, reading an opened one is a synchronous ABI
//! call, so a query lives in two beats: on_query dispatches the reads, the
//! replies arrive in on_read_result, and the last of them closes the query
//! with a terminal QueryDone carrying the list of misses. A miss is not an
//! error: it sends the client to the producer (image-tiler).

use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;

use crate::module::{layout, store, tile, State};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileAddr, TileResult};


/// A query in flight.
pub struct Query {
    /// Who receives texture ownership — the query's publisher.
    pub owner: String,
    pub level: u32,
    /// How many reads are still in flight; zero closes the query.
    pub remaining: u32,
    pub misses: Vec<(u32, u32)>,
    pub label: String,
}

/// Context of a single read: whose tile it is and which one.
pub struct TileRead {
    pub query: String,
    pub x: u32,
    pub y: u32,
}

pub fn on_query(state: &mut State, req: QueryRequest) {
    let correlation = veldsdk::correlation();
    let fail = |error: String| {
        veldsdk::log::warn!(target: "handlers", "cache query: {}", error);
        crate::emit::on_query_done(&QueryDone { misses: Vec::new(), error }, &correlation);
    };

    let owner = match veldsdk::resource::requester("tile-cache/on_query") {
        Ok(owner) => owner,
        Err(e) => return fail(e),
    };
    if !layout::valid_key(&req.fingerprint) {
        return fail(format!("invalid cache key: '{}'", req.fingerprint));
    }
    if req.tiles.len() > tile::MAX_QUERY_TILES {
        return fail(format!("{} tiles at once — over the ceiling of {}", req.tiles.len(), tile::MAX_QUERY_TILES));
    }
    if req.tiles.is_empty() {
        crate::emit::on_query_done(&QueryDone { misses: Vec::new(), error: String::new() }, &correlation);
        return;
    }

    // Using the cache is a reason to refresh the source: by the marker's mtime
    // eviction tells the alive apart from the abandoned.
    store::touch(state, &req.fingerprint);

    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };
    state.queries.insert(correlation.clone(), Query {
        owner,
        level: req.level,
        remaining: req.tiles.len() as u32,
        misses: Vec::new(),
        label,
    });

    for tile in &req.tiles {
        let read = state
            .pending_reads
            .begin(TileRead { query: correlation.clone(), x: tile.x, y: tile.y });
        crate::calls::fs::on_read(&FsReadRequest {
            path: layout::tile_path(&req.fingerprint, req.level, tile.x, tile.y),
        }, &read);
    }
}

/// The fs reply to a tile read. No file is a miss; a broken file is a miss
/// too: the producer rewrites it fresh, and the cache fixes itself.
pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let Some(read) = state.pending_reads.take(&veldsdk::correlation()) else {
        return veldsdk::resource::discard("fs/on_read_result", opened);
    };
    let Some(query) = state.queries.get_mut(&read.query) else {
        // A query is not removed until the last reply, so this place is
        // unreachable — but the resource in the reply is still ours to handle.
        if let Some(handle) = opened.handle {
            veldsdk::resource::release(handle);
        }
        return;
    };

    match serve(&query.owner, query.level, read.x, read.y, &opened) {
        Ok(tile) => crate::emit::on_tile(&tile, &read.query),
        Err(miss) => {
            // A miss and a failure look the same to the client — it simply
            // goes to the producer, — but the log must tell them apart: "no
            // file" is ordinary business, while "the file is there but no tile
            // came out of it" the producer does not heal. It rewrites a good
            // file, the client asks again, and the failure repeats — with the
            // difference that each round costs a full pass over the source.
            //
            // This is asked of the course of events itself, not of the error
            // text: the filesystem does the opening, and its text differs on
            // every operating system (the same rule as in `fs::delete`).
            match miss {
                Miss::NoFile(why) => veldsdk::log::debug!(target: "handlers",
                    "{}: tile {}:{}:{} — miss: {}", query.label, query.level, read.x, read.y, why),
                Miss::Broken(why) => veldsdk::log::warn!(target: "handlers",
                    "{}: tile {}:{}:{} is there, but no tile came out of it: {}",
                    query.label, query.level, read.x, read.y, why),
            }
            query.misses.push((read.x, read.y));
        }
    }

    query.remaining -= 1;
    if query.remaining == 0 {
        let query = state.queries.remove(&read.query).expect("the query was just there");
        crate::emit::on_query_done(&QueryDone {
            misses: query.misses.into_iter().map(|(x, y)| TileAddr { x, y }).collect(),
            error: String::new(),
        }, &read.query);
    }
}

/// Why a tile was not served. What differs here is not the texts but the
/// places: opening is not our business and usually means "this tile does not
/// exist yet"; everything else we do ourselves, and there a failure is a
/// failure.
enum Miss {
    /// The file did not open: most often it simply does not exist.
    NoFile(String),
    /// The file is there but no tile came out of it: it did not parse, the
    /// texture was not allocated, or it did not upload.
    Broken(String),
}

/// File opened → bytes → RGBA → texture with ownership handed to the client.
fn serve(owner: &str, level: u32, x: u32, y: u32, opened: &ResourceOpened) -> Result<TileResult, Miss> {
    let handle = veldsdk::resource::accept(opened).map_err(Miss::NoFile)?;
    // The same ceiling as on write: a tile body is compressed, and compressed
    // is never larger than expanded.
    let bytes = veldsdk::resource::read_whole(handle, layout::MAX_TILE_BYTES as u64)
        .map_err(Miss::Broken)?;

    let (width, height, rgba) = decode(&bytes).map_err(Miss::Broken)?;
    let texture = veldsdk::graphics::upload_texture(
        "tile", width, height, tile::TILE_FORMAT, &rgba, owner,
    )
        .map_err(Miss::Broken)?;
    Ok(TileResult { level, x, y, texture: Some(texture), width, height })
}

/// Encoded tile → RGBA8. Three-channel ones are expanded: the codec stores as
/// many channels as there were at write time, while the texture needs four.
fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    // Sizes are asked of the header, not of the decoded data. The decoder
    // allocates by them, and a check after it would reject what is already
    // allocated — while at the sizes it exists for, it would never run at all:
    // the codec has its own ceiling of four hundred megapixels, that is one
    // and a half gigabytes of RGBA, and the instance is not given that much.
    let header = qoi::decode_header(bytes).map_err(|e| format!("qoi: {}", e))?;
    let (width, height) = (header.width, header.height);
    if width == 0 || height == 0 || width > layout::MAX_TILE_SIDE || height > layout::MAX_TILE_SIDE {
        return Err(format!("qoi: implausible dimensions {}×{}", width, height));
    }
    let (_, pixels) = qoi::decode_to_vec(bytes).map_err(|e| format!("qoi: {}", e))?;
    let pixel_count = (width as usize) * (height as usize);
    match pixels.len() / pixel_count.max(1) {
        4 => Ok((width, height, pixels)),
        3 => {
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for px in pixels.chunks_exact(3) {
                rgba.extend_from_slice(px);
                rgba.push(255);
            }
            Ok((width, height, rgba))
        }
        other => Err(format!("qoi: {} channels", other)),
    }
}
