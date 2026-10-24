use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;

use crate::module::{layout, store, tile, State};
use crate::proto::tile_cache::{QueryDone, QueryRequest, TileAddr, TileResult};

pub struct Query {
    pub owner: String,
    pub level: u32,
    pub remaining: u32,
    pub misses: Vec<(u32, u32)>,
    pub label: String,
}

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

pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let Some(read) = state.pending_reads.take(&veldsdk::correlation()) else {
        return veldsdk::resource::discard("fs/on_read_result", opened);
    };
    let Some(query) = state.queries.get_mut(&read.query) else {
        if let Some(handle) = opened.handle {
            veldsdk::resource::release(handle);
        }
        return;
    };

    match serve(&query.owner, query.level, read.x, read.y, &opened) {
        Ok(tile) => crate::emit::on_tile(&tile, &read.query),
        Err(miss) => {
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

/// Why a tile was not served. A miss and a failure look the same to the client
/// — it goes to the producer — but the log tells them apart: an absent file is
/// ordinary business, while a present but unreadable tile is not healed by the
/// producer's rewrite and every retry costs a full pass over the source. Asked
/// of the course of events, not of the error text: the filesystem words its
/// failures differently on every OS.
enum Miss {
    NoFile(String),
    Broken(String),
}

fn serve(owner: &str, level: u32, x: u32, y: u32, opened: &ResourceOpened) -> Result<TileResult, Miss> {
    let handle = veldsdk::resource::accept(opened).map_err(Miss::NoFile)?;
    let bytes = veldsdk::resource::read_whole(handle, layout::MAX_TILE_BYTES as u64)
        .map_err(Miss::Broken)?;

    let (width, height, rgba) = decode(&bytes).map_err(Miss::Broken)?;
    let texture = veldsdk::graphics::upload_texture(
        "tile", width, height, tile::TILE_FORMAT, &rgba, owner,
    )
        .map_err(Miss::Broken)?;
    Ok(TileResult { level, x, y, texture: Some(texture), width, height })
}

fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
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
