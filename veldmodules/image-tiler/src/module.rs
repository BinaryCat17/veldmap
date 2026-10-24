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
use veldmap_tile_cache_wrap::tile::TILE_FORMAT;

use crate::proto::image_tiler::{
    Described, DescribeRequest, GeoTie, Level, Placement, ProduceDone, ProduceProgress,
    ProduceRequest, Serve, TileResult, Variable,
};
use crate::proto::tile_cache::StoreTile;

const PROGRESS_STEP: u64 = 8 * 1024 * 1024;

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

struct Parsed {
    resource: u64,
    variable: String,
    info: adapters::Info,
    fingerprint: String,
}

const MEMO_SLOTS: usize = 2;

pub struct State {
    kept: Vec<Parsed>,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    Ok(State { kept: Vec::new() })
}

impl State {
    fn keep(&mut self, kept: Parsed) {
        self.kept.retain(|other| !(other.resource == kept.resource && other.variable == kept.variable));
        self.kept.insert(0, kept);
        self.kept.truncate(MEMO_SLOTS);
    }

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

    fn neighbour_footprint(&self, resource_id: u64, variable: &str) -> u64 {
        self.kept
            .iter()
            .filter(|kept| !(kept.resource == resource_id && kept.variable == variable))
            .map(|kept| kept.info.footprint())
            .sum()
    }

    fn kept(&self, resource_id: u64, variable: &str) -> Option<&Parsed> {
        self.kept.iter().find(|kept| kept.resource == resource_id && kept.variable == variable)
    }
}

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

fn wire_variable(variable: &adapters::Variable) -> Variable {
    Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

fn describe(state: &mut State, req: &DescribeRequest) -> Result<Described, String> {
    let resource = req.resource.clone().ok_or_else(|| "no resource in the request".to_string())?;
    let began = Instant::now();
    let mut ties = Vec::new();
    let (kept, stamped, _) = parsed(state, resource.id, resource.size, &req.variable, &Rc::new(Cell::new(0)))?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());
    let read = began.elapsed();

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
    let owner = veldsdk::resource::requester("image-tiler/on_produce")?;
    let resource = req.resource.clone().ok_or_else(|| "no resource in the request".to_string())?;

    let bytes = Rc::new(Cell::new(0u64));
    let (kept, _, neighbour) = parsed(state, resource.id, resource.size, &req.variable, &bytes)?;
    let (info, fingerprint) = (&kept.info, kept.fingerprint.clone());

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

    if !sink.wants.is_empty() {
        return Err(format!("adapter did not produce {} of the requested tiles", sink.wants.len()));
    }
    Ok(())
}

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

    let mut ordered: Vec<(u32, u32)> = wants.into_iter().collect();
    ordered.sort_by_key(|&(x, y)| (y, x));
    Ok(ordered)
}

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
