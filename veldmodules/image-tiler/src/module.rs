//! image-tiler: a raster resource → pyramid tiles (RGBA8 sRGB).
//!
//! Where the bytes come from is not its concern: the requester opens the
//! resource itself and grants a read, and the bytes are read by windows the
//! same way — file, network, or memory. Durable state is the tile cache, owned
//! by tile-cache, and only its misses arrive here. So a kill in the middle of
//! the longest pass unwinds nothing: tiles already handed out are with the
//! requester, tiles already sent are in the cache, the rest is honestly lost.
//!
//! The module holds no state an answer would depend on; it keeps the source
//! parse between requests (see [`Parsed`]) — but this is memo, not memory: the
//! answer does not change with it, only the cost does.
//!
//! module.rs is the facade: State, init, handlers, and the tile sink (Sink).
//! Pyramid arithmetic lives in pyramid.rs (shared with consumers via the wrap),
//! the pass in cascade.rs, the formats in adapters/, identity in fingerprint.rs.

pub mod adapters;
pub mod budget;
pub mod cascade;
pub mod fingerprint;
pub mod pyramid;
pub mod resample;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use veldsdk::graphics as gfx;
// The tile format is not ours: it is shared with the cache and declared where
// both sides see it (see `tile.rs` in tile-cache). A private copy here would
// eventually diverge from it.
use veldmap_tile_cache_wrap::tile::TILE_FORMAT;

use crate::proto::image_tiler::{
    Described, DescribeRequest, GeoTie, Level, Placement, ProduceDone, ProduceProgress,
    ProduceRequest, Serve, TileResult, Variable,
};
use crate::proto::tile_cache::StoreTile;

/// Progress step over bytes read: more often it is noise, less often it
/// appears stuck.
const PROGRESS_STEP: u64 = 8 * 1024 * 1024;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

/// A parsed source, kept until the next request.
///
/// A request is served entirely in the handler, and there is still no state an
/// answer would depend on here: this is memo, not memory. A kill in the middle
/// of a pass loses it along with the instance, and the next request simply
/// parses the file again — the answer does not change, only the cost does.
///
/// Cost is the reason. `describe` and `produce` come as a pair for one and the
/// same resource, and a single description is followed by as many passes as
/// the source has levels; a parse is headers and catalogues, and for NetCDF
/// also the window sampling of the variable (`netcdf::describe`), and over the
/// network each one costs round trips to the block pool. The parse holds no
/// samples for any format: they are read by windows on demand, and there is
/// nothing to keep in memo but the layout.
struct Parsed {
    resource: u64,
    /// The named variable (`DescribeRequest.variable`); empty — the adapter's
    /// choice. Part of the memo key: the same resource with another variable is
    /// another parse, and taking a foreign one as ready would hand out its
    /// samples under its own fingerprint.
    variable: String,
    info: adapters::Info,
    /// Key of this source in the tile cache. It sits next to the parse because
    /// it is computed from the same file read: head and tail of 64 KiB each,
    /// that is, two round trips to the block pool for a remote resource. The
    /// parse outlives dozens of orders (a ladder level, a revealed edge), and
    /// the fingerprint must outlive as many: it is a property of the file, not
    /// of the order.
    fingerprint: String,
}

/// How many parses sit in memo. Two, because rasters come in interleaved
/// pairs — described the quicklook, described the granule, produced the
/// quicklook, produced the granule — and with a single slot each would evict
/// the other: the quicklook parse would not survive even its own pass. More is
/// needless: the tiler has two requesters, and each keeps one pair in view.
const MEMO_SLOTS: usize = 2;

pub struct State {
    /// Parses from newest to oldest; the extra drops off the tail.
    kept: Vec<Parsed>,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { kept: Vec::new() })
}

impl State {
    /// Keep a parse as fresh; the oldest beyond [`MEMO_SLOTS`] drops out.
    fn keep(&mut self, kept: Parsed) {
        self.kept.retain(|other| !(other.resource == kept.resource && other.variable == kept.variable));
        self.kept.insert(0, kept);
        self.kept.truncate(MEMO_SLOTS);
    }

    /// Mark the parse of this resource with this variable as fresh; `false` —
    /// it is not in memo.
    fn touch(&mut self, resource_id: u64, variable: &str) -> bool {
        match self.kept.iter().position(|kept| kept.resource == resource_id && kept.variable == variable) {
            Some(at) => {
                let kept = self.kept.remove(at);
                self.kept.insert(0, kept);
                true
            }
            None => false,
        }
    }

    /// How much memory the foreign parses sitting in memo hold — of another
    /// resource or of the same one with another variable: work on one's own
    /// runs alongside them, and its peak must count them as a term.
    fn neighbour_footprint(&self, resource_id: u64, variable: &str) -> u64 {
        self.kept
            .iter()
            .filter(|kept| !(kept.resource == resource_id && kept.variable == variable))
            .map(|kept| kept.info.footprint())
            .sum()
    }

    /// The parse of this resource with this variable, if it sits in memo.
    fn kept(&self, resource_id: u64, variable: &str) -> Option<&Parsed> {
        self.kept.iter().find(|kept| kept.resource == resource_id && kept.variable == variable)
    }
}

/// Parse of the source — from memo if this is the same resource, otherwise
/// fresh; with it, how long the fingerprint took and how much memory the
/// neighbouring parses left in memo hold ([`State::neighbour_footprint`]): once
/// the parse is taken by reference, `state` itself can no longer be asked.
///
/// It is handed out as a reference, not a value: a single description is
/// followed by as many passes as the source has levels (four or five for a
/// tiled TIFF), and a parse consumed by the first of them would save exactly
/// one.
fn parsed<'a>(
    state: &'a mut State,
    resource_id: u64,
    size: u64,
    variable: &str,
    bytes: &Rc<Cell<u64>>,
) -> Result<(&'a Parsed, Duration, u64), String> {
    let mut stamped = Duration::ZERO;
    if state.touch(resource_id, variable) {
        veldsdk::log::debug!(target: "decode", "parse of resource {} taken ready", resource_id);
    } else {
        veldsdk::log::debug!(target: "decode", "parsing resource {} ({} bytes){}", resource_id, size,
            match variable.is_empty() { true => String::new(), false => format!(", variable '{}'", variable) });
        let began = Instant::now();
        let fingerprint = fingerprint::fingerprint(resource_id, size, variable)?;
        stamped = began.elapsed();
        let info = adapters::describe(resource_id, size, variable, bytes)?;
        state.keep(Parsed { resource: resource_id, variable: variable.to_string(), info, fingerprint });
    }
    let neighbour = state.neighbour_footprint(resource_id, variable);
    Ok((state.kept(resource_id, variable).expect("parse was just kept"), stamped, neighbour))
}

pub fn on_describe(state: &mut State, req: DescribeRequest) {
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    // A failed description leaves no per-step layout — it is printed at the end
    // of a successful one. And failing can be expensive: an expired signature
    // refuses after three attempts with pauses, and in a silent log this wait
    // looks the same as a successful one.
    let began = Instant::now();
    let described = match describe(state, &req) {
        Ok(described) => described,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {} (in {:.2} s)",
                                label, error, began.elapsed().as_secs_f32());
            Described { error, ..Default::default() }
        }
    };
    crate::emit::on_described(&described, &correlation);
}

/// The variable onto the wire — as is.
fn wire_variable(variable: &adapters::Variable) -> Variable {
    Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

fn describe(state: &mut State, req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "no resource in the request".to_string())?;
    let began = Instant::now();
    // The binding arrives from a neighbouring file and does not go into memo: it
    // is a property of the pair "raster and its coordinates", and memo knows
    // only about the raster.
    let mut ties = Vec::new();
    let (kept, stamped, _) = parsed(state, resource.id, resource.size, &req.variable, &Rc::new(Cell::new(0)))?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());
    let read = began.elapsed();

    // Coordinates from a neighbouring file — only when the raster itself has
    // none: what is written in the raster knows its own layout better than any
    // neighbour.
    //
    // A failure here does not cancel the description. The raster is not spoiled
    // by it, it merely stays unbound — and what to do with it then is the
    // requester's decision: they have the catalogue outline, and we have
    // nothing. But silence about it is not an option either: by an empty
    // binding the requester cannot tell "the files do not say" from "they say,
    // but it did not read".
    let mut sidecar = None;
    if info.ties.is_empty()
        && info.placement.is_none()
        && let Some(coordinates) = req.geolocation.as_ref()
    {
        match adapters::netcdf::geolocation(
            coordinates.id,
            coordinates.size,
            info.frame,
            info.width,
            info.height,
        ) {
            Ok(found) => ties = found,
            Err(error) => {
                veldsdk::log::warn!(target: "decode", "coordinates file: {}", error);
                sidecar = Some(format!("coordinates file: {}", error));
            }
        }
    }

    // The description is the longest and the most silent part of a show: it has
    // no progress at all (see schema.yaml), and inside it lie the fingerprint
    // with both ends of the file, the parse, and the neighbouring coordinates
    // file.
    //
    // The fingerprint's seconds are the network, and the parse is paid for with
    // the same ones: the file head is needed by both, and after the fingerprint
    // it sits in the host's block cache. Reading "fingerprint 2.6 — parse 0.0"
    // as "the parse is cheap" is wrong.
    //
    // Time only: how much went over the wire is not visible from here — the
    // `Metered` counter counts the farthest reached position, not the volume,
    // and the wire is measured by the host (`network::perf`).
    // The answer carries a binding — the raster's own or one arrived from a
    // neighbouring file.
    let placed = !info.ties.is_empty() || info.placement.is_some() || !ties.is_empty();

    let total = began.elapsed();
    veldsdk::log::debug!(target: "perf",
        "description of resource {}: {:.2} s — fingerprint {:.2}, parse {:.2}, coordinates {:.2}",
        resource.id, total.as_secs_f32(), stamped.as_secs_f32(),
        (read - stamped).as_secs_f32(), (total - read).as_secs_f32());

    Ok(Described {
        fingerprint,
        width: info.width,
        height: info.height,
        tile: pyramid::TILE,
        // The level table is the same one `produce` picks its sleeve by: a row
        // per level, exactly as computed, without a retelling in scalars.
        levels: info
            .levels()
            .iter()
            .map(|row| Level {
                serve: match row.serve {
                    adapters::table::Serve::Pointwise => Serve::Pointwise,
                    adapters::table::Serve::Pass { .. } => Serve::Pass,
                } as i32,
                from: match row.serve {
                    adapters::table::Serve::Pass { from } => from,
                    adapters::table::Serve::Pointwise => 0,
                },
                bytes: row.peak.total(),
                fits: row.fits,
            })
            .collect(),
        ties: info
            .ties
            .iter()
            .chain(ties.iter())
            .map(|tie| GeoTie { px: tie.px, py: tie.py, lat: tie.lat, lon: tie.lon })
            .collect(),
        placement: info.placement.as_ref().map(|found| Placement {
            epsg: found.epsg,
            x_per_i: found.affine[0],
            x_per_j: found.affine[1],
            x0: found.affine[2],
            y_per_i: found.affine[3],
            y_per_j: found.affine[4],
            y0: found.affine[5],
        }),
        error: String::new(),
        variable: info.variable.as_ref().map(wire_variable),
        variables: info.variables.iter().map(wire_variable).collect(),
        // The caveat travels exactly when there is no binding in the answer: the
        // field promises "the binding could not be taken", and sent along with a
        // binding that was taken it would call trouble where there is none. This
        // is caught here, not by the consumer: the adapter knows nothing of the
        // neighbouring coordinates file, and that file precisely cancels its
        // complaint when it handled things itself.
        //
        // Both halves, not the major one: they are about different files, and
        // the second does not cancel the first. Each names its own — otherwise
        // the joined text reads as a repetition, because their trouble can be
        // one and the same.
        binding_trouble: match placed {
            true => String::new(),
            false => [
                info.binding_trouble.as_ref().map(|said| format!("raster: {}", said)),
                sidecar,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join("; "),
        },
    })
}

pub fn on_produce(state: &mut State, req: ProduceRequest) {
    // The request correlation is also the operation name at the platform: with
    // it the requester kills the operation when the tiles are no longer needed.
    let correlation = veldsdk::correlation();
    let label = if req.label.is_empty() { correlation.clone() } else { req.label.clone() };

    let began = Instant::now();
    let error = match produce(state, &req, &correlation) {
        Ok(()) => String::new(),
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: {}", label, error);
            error
        }
    };
    // The pass is the tiler's longest job, and there is nothing else left to
    // measure it with: the description prints its own layout, while what the
    // decoding itself cost never followed from the logs. The numbers are the
    // same ones the pass reports its progress by: without the level and the
    // tile count the seconds have nothing to compare against — a coarse level
    // and a detailed one are different work.
    //
    // A failure is measured on par with a success: a human waited out its
    // seconds just the same, and in silence a long refusal is indistinguishable
    // from a quick one.
    veldsdk::log::debug!(target: "perf",
        "pass over resource {}: {:.2} s — level {}, tiles {}{}",
        req.resource.as_ref().map_or(0, |handle| handle.id),
        began.elapsed().as_secs_f32(),
        req.level,
        req.tiles.len(),
        match error.is_empty() {
            true => String::new(),
            false => format!(", failed: {}", error),
        });
    crate::emit::on_produce_done(&ProduceDone { error }, &correlation);
}

fn produce(state: &mut State, req: &ProduceRequest, correlation: &str) -> Result<(), String> {
    // The owner of the future textures is whoever sent the request: without a
    // name there is no one to hand them to.
    let owner = veldsdk::resource::requester("image-tiler/on_produce")?;
    let resource = req.resource.clone().ok_or_else(|| "no resource in the request".to_string())?;

    let bytes = Rc::new(Cell::new(0u64));
    let (kept, _, neighbour) = parsed(state, resource.id, resource.size, &req.variable, &bytes)?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());

    // The work peak is checked against the free memory before it — together
    // with the neighbour's parse, which sits in memo and holds its memory. The
    // table row is the same one the description promised "fits" by; a refusal
    // here names the terms.
    let admitted = match info.level(req.level) {
        Some(row) => row.peak.with("neighbour parse", neighbour).admit(),
        None => Ok(()),
    };
    let ordered = plan(info, req)?;
    admitted?;

    let wants: BTreeSet<(u32, u32)> = ordered.iter().copied().collect();
    let mut sink = Sink {
        correlation,
        owner,
        fingerprint,
        want_level: req.level,
        wants,
        want_total: ordered.len() as u32,
        done: 0,
        bytes: bytes.clone(),
        total_bytes: resource.size,
        reported: 0,
    };
    {
        let mut emit = |level: u32, tx: u32, ty: u32, w: u32, h: u32, rgba: &[u8]| {
            sink.emit(level, tx, ty, w, h, rgba)
        };
        adapters::produce(resource.id, resource.size, info, req.level, &ordered, &bytes, &mut emit)?;
    }

    // The pass ended but not everything requested is delivered — this is an
    // adapter error, and staying silent about it means showing the requester a
    // hole without a reason.
    if !sink.wants.is_empty() {
        return Err(format!("adapter did not produce {} of the requested tiles", sink.wants.len()));
    }
    Ok(())
}

/// Which cells are ordered — with a check that such cells exist at all.
///
/// A pure function over the description and the request: the rule "the level
/// exists, the cell is in its grid" depends on neither the source nor the
/// module state.
fn plan(info: &adapters::Info, req: &ProduceRequest) -> Result<Vec<(u32, u32)>, String> {
    let levels = pyramid::level_count(info.width, info.height);
    if req.level >= levels {
        return Err(format!("there is no level {}: the raster has {} of them", req.level, levels));
    }
    let grid_w = pyramid::grid(pyramid::level_size(info.width, req.level));
    let grid_h = pyramid::grid(pyramid::level_size(info.height, req.level));
    let mut wants = BTreeSet::new();
    for tile in &req.tiles {
        if tile.x >= grid_w || tile.y >= grid_h {
            return Err(format!(
                "there is no tile {}:{}: the grid of level {} is {}×{}",
                tile.x, tile.y, req.level, grid_w, grid_h
            ));
        }
        wants.insert((tile.x, tile.y));
    }

    // By rows, not by columns. Chunks of a tiled TIFF are addressed as
    // `cy * across + cx`, that is, they lie in rows, and the host's readahead
    // recognizes a sequential pass by a miss landing right after the end of the
    // previous request. A column-wise walk jumps over the whole width of the
    // chunk grid on every tile, and the ramp-up resets to a single block.
    //
    // The order is handed out from here, not arranged over the finished set:
    // there it can be forgotten, and the order would silently travel by columns
    // again.
    //
    // It removes not all jumps, but only those between tiles. Inside a tile the
    // area is assembled by a loop over chunk rows, and when a chunk lies below
    // this area — in a file with internal tiles 256×256 against our 512 — the
    // next tile of the row returns to its first chunk row. There the gain drops
    // from seventy percent to single digits, and that is no longer cured by the
    // order of the request. For the same reason the gain stands on a level being
    // read from its own copy: from a copy twice as large one tile covers 2×2
    // chunks, and the return happens on every one.
    //
    // This gain is in the number of requests, not in bytes. Ramped-up reading
    // takes a piece ahead of time, and at a row break the tail slides past the
    // right edge of the window, where no one asks for it: with a wide window
    // there are half as many requests for the same bytes, with a narrow and
    // tall one up to one and a half times more bytes for the same number of
    // requests. The pool meanwhile is shared per process, and the extra in it
    // evicts someone else's paid-for blocks — visible in the `network::perf`
    // line.
    //
    // And the rule itself is about the chunk index, not about its place in the
    // file: the physical order is set by `TileOffsets`, and it is monotonic only
    // by the writer's custom. A file rebuilt by a foreign tool brings the jumps
    // back, and it can be noticed only by the same counter.
    let mut ordered: Vec<(u32, u32)> = wants.into_iter().collect();
    ordered.sort_by_key(|&(x, y)| (y, x));
    Ok(ordered)
}

/// Sink for produced tiles: every one goes to the cache, the requested ones
/// also to the requester as a texture. Progress travels from here too: the
/// adapters have neither a correlation nor knowledge of who expected what.
struct Sink<'a> {
    correlation: &'a str,
    owner: String,
    fingerprint: String,
    want_level: u32,
    wants: BTreeSet<(u32, u32)>,
    want_total: u32,
    done: u32,
    bytes: Rc<Cell<u64>>,
    total_bytes: u64,
    reported: u64,
}

impl Sink<'_> {
    fn emit(&mut self, level: u32, tx: u32, ty: u32, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
        // The edge of transparent pixels is filled with the neighbour's color
        // here, not in the adapters: all tiles of all sleeves converge here, and
        // the halo at the `nodata` boundary is the same for each (see
        // `adapters::bleed_alpha`). A copy is made only where transparency
        // exists at all — an ordinary scene has none.
        let bled;
        let rgba = match rgba.chunks_exact(4).any(|pixel| pixel[3] == 0) {
            false => rgba,
            true => {
                let mut copy = rgba.to_vec();
                adapters::bleed_alpha(&mut copy, w, h);
                bled = copy;
                &bled
            }
        };

        // Into the cache goes everything produced: a second request for this
        // source must be served without a pass. Fire-and-forget, no
        // confirmation is awaited.
        let encoded = qoi::encode_to_vec(rgba, w, h).map_err(|e| format!("qoi: {}", e))?;
        crate::calls::tile_cache::on_store(&StoreTile {
            fingerprint: self.fingerprint.clone(),
            level,
            x: tx,
            y: ty,
            qoi: encoded,
        });

        if level == self.want_level && self.wants.remove(&(tx, ty)) {
            let texture = gfx::upload_texture("tile", w, h, TILE_FORMAT, rgba, &self.owner)?;
            crate::emit::on_produced(
                &TileResult { level, x: tx, y: ty, texture: Some(texture), width: w, height: h },
                self.correlation,
            );
            self.done += 1;
            self.progress(true);
        } else {
            self.progress(false);
        }
        Ok(())
    }

    /// Progress: always on a delivered tile, and between them by megabytes of
    /// the source read (a long pass is visible also where no requested tile has
    /// come yet).
    fn progress(&mut self, force: bool) {
        let read = self.bytes.get();
        if !force && read.saturating_sub(self.reported) < PROGRESS_STEP {
            return;
        }
        self.reported = read;
        crate::emit::on_produce_progress(
            &ProduceProgress {
                read_bytes: read,
                total_bytes: self.total_bytes,
                done_tiles: self.done,
                want_tiles: self.want_total,
            },
            self.correlation,
        );
    }
}
