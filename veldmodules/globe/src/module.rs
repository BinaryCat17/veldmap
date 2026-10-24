pub mod camera;
pub mod cull;
pub mod geodesy;
pub mod gpu;
pub mod mesh;
pub mod outlines;
pub mod overlay;
pub mod perf;
pub mod projection;
pub mod wheel;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use camera::Camera;
use gpu::{Device, Target};
use overlay::{Overlay, Raster, Role};
use veldmap_image_tiler_wrap::tiles::{self, Addr, Missed, Passes, Store};
use veldsdk::proto::app as app_proto;
use veldsdk::proto::core::SurfaceDelegated;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest, TileAddr,
    TileResult as ProducedTile,
};
use crate::proto::globe::{OverlayProgress, OverlaysProgress};
use crate::proto::tile_cache::{
    QueryDone, QueryRequest, TileAddr as QueryAddr, TileResult as CachedTile,
};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
}

pub struct QueryCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct ProduceCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct State {
    camera: Camera,
    device: Option<Device>,
    target: Option<Target>,
    drawn: Option<Frame>,
    phase: f32,
    ripple: Option<f32>,
    outlines: Vec<crate::proto::globe::Outline>,
    generation: u64,

    overlays: Vec<Overlay>,
    refused: Vec<(String, String)>,
    tiles: Store,
    passes: Passes<(String, Role)>,
    batch: gpu::OverlayBatch,
    patch_vertices: Vec<gpu::OverlayVertex>,
    built: Option<Vec<overlay::Built>>,
    patches: u64,
    epoch: u64,
    checked: Option<(overlay::Look, u64, u64)>,
    pending_describe: veldsdk::Correlator<(String, Role)>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    perf: perf::Meter,
}

#[derive(Clone, Copy, PartialEq)]
struct Frame {
    camera: Camera,
    texture: u64,
    generation: u64,
    patches: u64,
    ripple: u32,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        camera: Camera::default(),
        device: None,
        target: None,
        drawn: None,
        phase: 0.0,
        ripple: None,
        outlines: Vec::new(),
        generation: 0,
        overlays: Vec::new(),
        refused: Vec::new(),
        tiles: Store::new(config.vram_budget_mb * 1024 * 1024),
        passes: Passes::default(),
        batch: gpu::OverlayBatch::new(),
        patch_vertices: Vec::new(),
        built: None,
        patches: 0,
        epoch: 0,
        checked: None,
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
        perf: perf::Meter::default(),
    })
}

pub fn on_set_surface(state: &mut State, req: SurfaceDelegated) {
    let Some(surface) = req.surface else {
        veldsdk::log::info!(target: "handlers", "space revoked");
        state.target = None;
        report_progress(state, &[]);
        return;
    };
    if req.width == 0 || req.height == 0 {
        veldsdk::log::warn!(target: "handlers", "space {}x{} — nothing to draw on", req.width, req.height);
        return;
    }

    if state.device.as_ref().is_none_or(|device| device.format != req.format) {
        match Device::create(req.format) {
            Ok(device) => {
                state.tiles = Store::new_like(&state.tiles);
                state.built = None;
                state.checked = None;
                state.device = Some(device);
            }
            Err(error) => {
                veldsdk::log::error!(target: "handlers", "device resources: {:#}", error);
                state.target = None;
                return;
            }
        }
    }

    upload_outlines(state);

    match Target::create(surface.id, req.width, req.height) {
        Ok(target) => {
            veldsdk::log::info!(target: "handlers",
                "space: texture {} ({}x{})", surface.id, req.width, req.height);
            state.target = Some(target);
        }
        Err(error) => {
            veldsdk::log::error!(target: "handlers", "depth buffer: {:#}", error);
            state.target = None;
        }
    }
    want_tiles(state, perf::Pass::Set);
}

pub fn on_camera(state: &mut State, command: crate::proto::globe::CameraCommand) {
    use crate::proto::globe::camera_command::Command;
    match command.command {
        Some(Command::Orbit(orbit)) => state.camera.orbit(orbit.dx, orbit.dy),
        Some(Command::Zoom(zoom)) => {
            state.camera.zoom_at(zoom.delta, zoom.hold_x, zoom.hold_y)
        }
        Some(Command::Focus(focus)) => match focus.at {
            Some(at) => state.camera.focus(at.lat, at.lon, focus.radius_deg),
            None => veldsdk::log::warn!(target: "handlers", "focus without a point"),
        },
        None => {}
    }
    want_tiles(state, perf::Pass::Camera);
}

pub fn on_probe(state: &mut State, probe: crate::proto::globe::Probe) {
    let at = state.target.as_ref().and_then(|target| {
        state
            .camera
            .probe(probe.x, probe.y, target.aspect())
            .map(|(lat, lon)| crate::proto::globe::GeoPoint { lat, lon })
    });

    crate::emit::on_probed(
        &crate::proto::globe::Probed { at },
        &veldsdk::correlation(),
    );
}

pub fn on_outlines(state: &mut State, outlines: crate::proto::globe::Outlines) {
    state.outlines = outlines.outlines;
    state.generation += 1;
    upload_outlines(state);
}

pub fn on_overlay(state: &mut State, msg: crate::proto::globe::Overlays) {
    let keep: HashSet<String> = msg.overlays.iter().map(|o| o.key.clone()).collect();
    state.refused.clear();

    let removed: Vec<Overlay> = {
        let (kept, removed) =
            state.overlays.drain(..).partition(|overlay| keep.contains(&overlay.key));
        state.overlays = kept;
        removed
    };
    for overlay in removed {
        veldsdk::log::info!(target: "handlers", "{}: overlay removed", overlay.label);
        drop_overlay(state, overlay);
    }

    let order: Vec<String> = msg.overlays.iter().map(|overlay| overlay.key.clone()).collect();
    for incoming in msg.overlays {
        adopt_overlay(state, incoming);
    }
    state.overlays.sort_by_key(|overlay| {
        order.iter().position(|key| key == &overlay.key).unwrap_or(usize::MAX)
    });

    state.epoch += 1;
    want_tiles(state, perf::Pass::Set);
}

fn cap_tiles(state: &State) -> u64 {
    state.tiles.cap_tiles(
        state
            .overlays
            .iter()
            .filter(|overlay| !overlay.hidden)
            .flat_map(|overlay| overlay.budgeted())
            .filter_map(|raster| raster.meta.as_ref())
            .map(|meta| meta.fingerprint.as_str()),
    )
}

fn look(incoming: &crate::proto::globe::Overlay) -> (f32, bool) {
    (incoming.opacity.unwrap_or(1.0).clamp(0.0, 1.0), incoming.hidden)
}

fn adopt_overlay(state: &mut State, incoming: crate::proto::globe::Overlay) {
    if incoming.key.is_empty() {
        veldsdk::log::warn!(target: "handlers", "overlay without a key — resources are released");
        release_rasters(incoming.rasters);
        return;
    }
    let (opacity, hidden) = look(&incoming);

    let incoming_ids: Vec<u64> = incoming
        .rasters
        .iter()
        .flat_map(|raster| [raster.resource.as_ref(), raster.geolocation.as_ref()])
        .flatten()
        .map(|handle| handle.id)
        .collect();
    let incoming_variables: Vec<String> = incoming.rasters.iter().map(|raster| raster.variable.clone()).collect();
    if let Some(index) = state.overlays.iter().position(|o| o.key == incoming.key) {
        if state.overlays[index].sources == incoming_ids {
            if state.overlays[index].variables != incoming_variables {
                let wanted: Vec<(u64, String)> = incoming
                    .rasters
                    .iter()
                    .filter_map(|raster| raster.resource.as_ref().map(|handle| (handle.id, raster.variable.clone())))
                    .collect();
                let key = incoming.key.clone();
                let roles = state.overlays[index].revariabled(&wanted);
                let label = state.overlays[index].label.clone();
                let mut requests = Vec::new();
                for role in roles {
                    let overlay = &mut state.overlays[index];
                    let Some(raster) = overlay.raster_mut(role) else { continue };
                    let variable = wanted
                        .iter()
                        .find(|(id, _)| *id == raster.resource.as_ref().id)
                        .map(|(_, variable)| variable.clone())
                        .unwrap_or_default();
                    let previous = raster.meta.as_ref().map(|meta| meta.fingerprint.clone());
                    raster.variable = variable.clone();
                    raster.meta = None;
                    raster.trouble = None;
                    raster.fetch.reset();
                    let correlation = raster.describe.begin();
                    veldsdk::log::info!(target: "handlers", "{}: variable '{}' — the raster is described anew", label, variable);
                    requests.push((role, previous, correlation, raster.resource.as_ref().clone(),
                        raster.geolocation.as_ref().map(|owned| owned.as_ref().clone()), variable));
                }
                state.overlays[index].variables = incoming_variables;
                for (role, previous, correlation, handle, coordinates, variable) in requests {
                    if let Some(fingerprint) = previous {
                        release_pass(state, &key, role, &fingerprint);
                        forget_unless_held(state, &fingerprint);
                    }
                    state.pending_describe.insert(correlation.clone(), (key.clone(), role));
                    crate::calls::image_tiler::on_describe(
                        &DescribeRequest { resource: Some(handle), label: label.clone(), geolocation: coordinates, variable },
                        &correlation,
                    );
                }
                state.epoch += 1;
            }
            let overlay = &mut state.overlays[index];
            let was = overlay.hidden;
            overlay.opacity = opacity;
            overlay.hidden = hidden;
            if hidden && !was {
                let sources: Vec<(Role, String)> = overlay
                    .rasters
                    .iter_mut()
                    .filter_map(|raster| {
                        raster.fetch.reset();
                        raster.meta.as_ref().map(|meta| (raster.role, meta.fingerprint.clone()))
                    })
                    .collect();
                let key = incoming.key.clone();
                for (role, fingerprint) in sources {
                    release_pass(state, &key, role, &fingerprint);
                }
                state.epoch += 1;
            }
            return;
        }
        let old = state.overlays.remove(index);
        drop_overlay(state, old);
    }

    let label = if incoming.label.is_empty() { incoming.key.clone() } else { incoming.label };

    let (frame, binding) = if let Some(utm) = incoming.utm {
        (
            overlay::Frame::utm(
                projection::System::utm(utm.zone, utm.south),
                utm.x0,
                utm.y0,
                utm.x1,
                utm.y1,
            ),
            overlay::Binding::Named,
        )
    } else if incoming.quad.len() == 4 {
        let points = [
            (incoming.quad[0].lat, incoming.quad[0].lon),
            (incoming.quad[1].lat, incoming.quad[1].lon),
            (incoming.quad[2].lat, incoming.quad[2].lon),
            (incoming.quad[3].lat, incoming.quad[3].lon),
        ];
        let frame = match incoming.rough {
            true => overlay::Frame::rough(points),
            false => overlay::Frame::quad(points),
        };
        (frame, overlay::Binding::Catalogue)
    } else {
        release_rasters(incoming.rasters);
        return refuse(state, incoming.key, &label, "the snapshot has nowhere to lie: no binding");
    };

    let mut rasters = Vec::new();
    for raster in incoming.rasters {
        let Some(handle) = raster.resource else {
            release_coordinates(raster.geolocation);
            continue;
        };
        let ordinal = raster.ordinal;
        let variable = raster.variable.clone();
        let role = match raster.role() {
            crate::proto::globe::OverlayRole::OverlayPreview => Role::Preview,
            crate::proto::globe::OverlayRole::OverlayDetailed => Role::Detailed,
        };
        if let Err(error) = veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
            veldsdk::log::warn!(target: "handlers", "{}: raster grant: {}", label, error);
            release_coordinates(raster.geolocation);
            continue;
        }
        let coordinates = raster.geolocation.filter(|handle| {
            match veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
                Ok(()) => true,
                Err(error) => {
                    veldsdk::log::warn!(target: "handlers",
                        "{}: coordinates grant: {}", label, error);
                    false
                }
            }
        });
        if let Some(first) = rasters.iter_mut().find(|raster: &&mut Raster| raster.role == role) {
            first.spares.push((
                ordinal,
                veldsdk::OwnedResource::new(handle),
                coordinates.map(veldsdk::OwnedResource::new),
            ));
            continue;
        }
        let mut raster = Raster::new(role, veldsdk::OwnedResource::new(handle.clone()));
        raster.ordinal = ordinal;
        raster.variable = variable.clone();
        raster.geolocation = coordinates.clone().map(veldsdk::OwnedResource::new);

        let correlation = raster.describe.begin();
        state.pending_describe.insert(correlation.clone(), (incoming.key.clone(), role));
        crate::calls::image_tiler::on_describe(
            &DescribeRequest {
                resource: Some(handle),
                label: label.clone(),
                geolocation: coordinates,
                variable,
            },
            &correlation,
        );
        rasters.push(raster);
    }
    if rasters.is_empty() {
        return refuse(state, incoming.key, &label, "no raster opened");
    }

    veldsdk::log::info!(target: "handlers", "{}: overlay of {} rasters", label, rasters.len());
    state.overlays.push(Overlay {
        key: incoming.key,
        label,
        frame,
        relaid: 0,
        binding,
        binding_trouble: None,
        rasters,
        sources: incoming_ids,
        variables: incoming_variables,
        opacity,
        hidden,
        error: String::new(),
        progress: overlay::Progress::default(),
    });
}

fn refuse(state: &mut State, key: String, label: &str, why: &str) {
    veldsdk::log::warn!(target: "handlers", "{}: {}", label, why);
    state.refused.push((key, why.to_string()));
}

fn forget_unless_held(state: &mut State, fingerprint: &str) {
    let held = state.overlays.iter().any(|other| {
        other.rasters.iter().any(|raster| raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint))
    });
    if !held {
        state.tiles.forget(fingerprint);
    }
}

fn drop_overlay(state: &mut State, mut overlay: Overlay) {
    let key = std::mem::take(&mut overlay.key);
    let sources: Vec<(Role, String)> = overlay
        .rasters
        .iter_mut()
        .filter_map(|raster| {
            raster.fetch.reset();
            raster.meta.as_ref().map(|meta| (raster.role, meta.fingerprint.clone()))
        })
        .collect();

    for (role, fingerprint) in sources {
        release_pass(state, &key, role, &fingerprint);
        forget_unless_held(state, &fingerprint);
    }
}

fn release_pass(state: &mut State, key: &str, role: Role, fingerprint: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &(key.to_string(), role)) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

fn release_coordinates(coordinates: Option<veldsdk::proto::core::ResourceHandle>) {
    if let Some(handle) = coordinates {
        veldsdk::resource::release(handle);
    }
}

fn release_rasters(rasters: Vec<crate::proto::globe::OverlayRaster>) {
    for raster in rasters {
        for handle in [raster.resource, raster.geolocation].into_iter().flatten() {
            veldsdk::resource::release(handle);
        }
    }
}

pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some((key, role)) = state.pending_describe.take(&correlation) else { return };
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
    if overlay.raster_mut(role).is_none_or(|raster| {
        raster.describe.settle(&correlation) != veldsdk::Reply::Current
    }) {
        return;
    }

    describe_settled(state, &key, role, msg);
    rebuild_bounds(state, &key);

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && !overlay.rasters.iter().any(|raster| raster.describe.is_pending())
    {
        if matches!(overlay.frame, overlay::Frame::Quad(_)) {
            veldsdk::log::warn!(target: "handlers",
                "{}: no binding taken from the rasters — the snapshot lies along the catalogue outline, the order of its vertices need not match the raster walk",
                overlay.label);
        }
        if overlay.doomed() {
            veldsdk::log::warn!(target: "handlers",
                "{}: no raster described — nothing to overlay", overlay.label);
            let said = overlay.trouble();
            overlay.error = match said.is_empty() {
                true => "no raster described".to_string(),
                false => said,
            };
        } else if !overlay.frame.measurable() {
            veldsdk::log::warn!(target: "handlers",
                "{}: the catalogue outline has no extent — there is nothing to lay the snapshot on",
                overlay.label);
            overlay.error = said_with(
                &overlay.binding_trouble,
                "the snapshot outline collapsed to a point: there is nothing to lay the raster on",
            );
        } else if matches!(overlay.frame, overlay::Frame::Rough(_)) {
            veldsdk::log::warn!(target: "handlers",
                "{}: the catalogue outline is more complex than a quadrilateral, and no binding was taken from the raster",
                overlay.label);
            overlay.error = said_with(
                &overlay.binding_trouble,
                "no binding: it could not be taken from the raster, and the catalogue outline is more complex than \
                 a quadrilateral",
            );
        }
    }

    state.epoch += 1;
    want_tiles(state, perf::Pass::Set);
}

fn said_with(trouble: &Option<String>, general: &str) -> String {
    match trouble {
        Some(said) => format!("{} ({})", general, said),
        None => general.to_string(),
    }
}

fn rebuild_bounds(state: &mut State, key: &str) {
    let Some(at) = state.overlays.iter().position(|o| o.key == key) else { return };
    let overlay = &state.overlays[at];
    let built: Vec<Vec<Vec<cull::Ball>>> = overlay
        .rasters
        .iter()
        .map(|raster| match &raster.meta {
            Some(meta) => overlay.bounds(meta),
            None => Vec::new(),
        })
        .collect();
    for (raster, bounds) in state.overlays[at].rasters.iter_mut().zip(built) {
        raster.bounds = bounds;
    }
}

fn describe_settled(state: &mut State, key: &str, role: Role, msg: Described) {
    let Some(mut overlay) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
    let label = overlay.label.clone();

    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: raster description: {}", label, error);
            let spare = overlay.raster_mut(role).and_then(|raster| {
                let next = raster.refused(error)?;
                Some((raster.describe.begin(), raster.spares.len(), next))
            });
            if let Some((correlation, left, (handle, coordinates))) = spare {
                let named = match role {
                    Role::Detailed => "detailed",
                    Role::Preview => "preview",
                };
                veldsdk::log::info!(target: "handlers",
                    "{}: spare raster ({}) — resource {}, {} more spares", label, named, handle.id, left);
                state.pending_describe.insert(correlation.clone(), (key.to_string(), role));
                crate::calls::image_tiler::on_describe(
                    &DescribeRequest { resource: Some(handle), label, geolocation: coordinates, variable: String::new() },
                    &correlation,
                );
                return;
            }
            return;
        }
    };

    veldsdk::log::info!(target: "handlers", "{}: {}", label, meta.note());

    let ties: Vec<overlay::Tie> = msg
        .ties
        .iter()
        .map(|tie| overlay::Tie {
            fx: tie.px / f64::from(msg.width),
            fy: tie.py / f64::from(msg.height),
            lat: tie.lat,
            lon: tie.lon,
        })
        .collect();

    let Some(raster) = overlay.raster_mut(role) else { return };
    raster.describe_as(meta);
    raster.trouble = None;

    let mut eclipsed = None;
    if overlay.detail_eclipsed()
        && let Some(raster) = overlay.raster_mut(Role::Detailed)
    {
        let said = "no more detailed than the preview: the detailed raster is not laid on the globe";
        if raster.trouble.as_deref() != Some(said) {
            veldsdk::log::info!(target: "handlers", "{}: {}", label, said);
        }
        raster.trouble = Some(said.to_string());
        eclipsed = raster.meta.as_ref().map(|meta| meta.fingerprint.clone());
        raster.fetch.reset();
    }
    if let Some(fingerprint) = eclipsed {
        release_pass(state, key, Role::Detailed, &fingerprint);
        let Some(again) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
        overlay = again;
    }

    if !ties.is_empty() {
        match overlay::Grid::new(&ties) {
            Some(grid) => {
                let rank = match grid.is_dense() {
                    true => overlay::Binding::Lattice,
                    false => overlay::Binding::Projected,
                };
                if overlay.binding >= rank {
                    return;
                }
                veldsdk::log::info!(target: "handlers",
                    "{}: binding by a grid of {} nodes", label, ties.len());
                overlay.relay(overlay::Frame::Grid(grid), rank);
                return;
            }
            None => {
                let said = format!("{} tie points did not assemble into a grid", ties.len());
                veldsdk::log::warn!(target: "handlers",
                    "{}: {} — the binding stays along the outline", label, said);
                overlay.complain(said);
            }
        }
    }

    let Some(found) = msg.placement else {
        if !msg.binding_trouble.is_empty() {
            veldsdk::log::warn!(target: "handlers",
                "{}: {} — the binding stays along the outline", label, msg.binding_trouble);
            overlay.complain(msg.binding_trouble);
        }
        return;
    };
    if overlay.binding >= overlay::Binding::Projected {
        return;
    }
    match overlay::Frame::from_placement(&overlay::Placement {
        epsg: found.epsg,
        x_per_i: found.x_per_i,
        x_per_j: found.x_per_j,
        x0: found.x0,
        y_per_i: found.y_per_i,
        y_per_j: found.y_per_j,
        y0: found.y0,
        width: msg.width,
        height: msg.height,
    }) {
        Ok(frame) => {
            veldsdk::log::info!(target: "handlers",
                "{}: binding by projection EPSG:{}", label, found.epsg);
            overlay.relay(frame, overlay::Binding::Projected);
        }
        Err(why) => {
            veldsdk::log::warn!(target: "handlers",
                "{}: the raster is bound to {} — the binding stays along the outline", label, why);
            overlay.complain(format!("binding not taken: {}", why));
        }
    }
}

pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);

    let cap = cap_tiles(state);
    let (toll, began) = (perf::Toll::default(), Instant::now());
    let desired: HashSet<Addr> = looking(state)
        .and_then(|look| {
            let overlay = state.overlays.iter().find(|o| o.key == ctx.key)?;
            Some(
                overlay
                    .wanted(&look, cap, &state.tiles, &toll)
                    .into_iter()
                    .filter(|wanted| wanted.choice.role == ctx.role)
                    .flat_map(|wanted| wanted.want.cells)
                    .collect(),
            )
        })
        .unwrap_or_default();
    state.perf.pass(perf::Pass::Answer, &toll, began.elapsed(), Instant::now());

    let Some((label, produce_list, resource, variable)) = ({
        let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) else { return };
        let label = overlay.label.clone();
        let hidden = overlay.hidden;
        let Some(raster) = overlay.raster_mut(ctx.role) else { return };
        if hidden {
            raster.fetch.forget_asked(&ctx.cells);
            return;
        }

        if raster.meta.as_ref().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
            return;
        }
        let error = veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone());
        if !error.is_empty() {
            raster.fetch.forget_asked(&ctx.cells);
            veldsdk::log::warn!(target: "handlers", "{}: tile cache: {}", label, error);
            raster.trouble = Some(format!("incomplete: {}", error));
            state.epoch += 1;
            return;
        }

        let pointwise =
            raster.meta.as_ref().is_some_and(|meta| meta.pointwise(level));
        let missed = raster.fetch.missed(
            &state.passes,
            &ctx.fingerprint,
            &(ctx.key.clone(), ctx.role),
            pointwise,
            &ctx.cells,
            msg.misses.iter().map(|addr| (level, addr.x, addr.y)),
            |addr| desired.contains(&addr),
        );
        match missed {
            Missed::Produce(cells) => Some((label, cells, raster.resource.handle(), raster.variable.clone())),
            Missed::Waiting => return,
            Missed::Closed => None,
        }
    }) else {
        state.epoch += 1;
        want_tiles(state, perf::Pass::Fetch);
        return;
    };

    let correlation = state.pending_produce.begin(ProduceCtx {
        key: ctx.key.clone(),
        role: ctx.role,
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    state.passes.begin(
        &ctx.fingerprint,
        (ctx.key.clone(), ctx.role),
        correlation.clone(),
        level,
        produce_list.clone(),
    );
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(resource),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label,
        variable,
    }, &correlation);
}

pub fn on_produce_progress(state: &mut State, msg: ProduceProgress) {
    veldsdk::log::debug!(target: "handlers", "production: {} of {} MiB, tiles {}/{}",
        msg.read_bytes >> 20, msg.total_bytes >> 20, msg.done_tiles, msg.want_tiles);

    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else { return };
    let (key, role) = (ctx.key.clone(), ctx.role);
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && let Some(raster) = overlay.raster_mut(role)
    {
        raster.pass = (msg.read_bytes, msg.total_bytes);
    }
    state.epoch += 1;
}

pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };
    let ours_kill = state.passes.finish(&correlation);

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) {
        let label = overlay.label.clone();
        if let Some(raster) = overlay.raster_mut(ctx.role) {
            let ours =
                raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
            let error = match ours_kill {
                true => msg.error.clone(),
                false => veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone()),
            };
            let failed = ours && !error.is_empty();
            raster.pass = (0, 0);
            raster.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
            if failed {
                veldsdk::log::warn!(target: "handlers", "{}: production: {}", label, error);
                raster.trouble = Some(format!("incomplete: {}", error));
            }
        }
    }

    state.epoch += 1;
    want_tiles(state, perf::Pass::Fetch);
}

fn want_tiles(state: &mut State, from: perf::Pass) {
    let Some(look) = looking(state) else { return };
    let cap = cap_tiles(state);

    let keys: Vec<String> = state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .map(|overlay| overlay.key.clone())
        .collect();
    let (toll, mut spent) = (perf::Toll::default(), Duration::ZERO);
    for key in keys {
        let began = Instant::now();
        let wanted = state
            .overlays
            .iter()
            .find(|o| o.key == key)
            .map(|overlay| overlay.wanted(&look, cap, &state.tiles, &toll))
            .unwrap_or_default();
        spent += began.elapsed();
        for wanted in wanted {
            want_overlay(state, &key, wanted);
        }
    }
    state.perf.pass(from, &toll, spent, Instant::now());
}

fn looking(state: &State) -> Option<overlay::Look> {
    let target = state.target.as_ref()?;
    Some(overlay::Look {
        view_proj: state.camera.view_projection(target.aspect()),
        eye: state.camera.eye(),
        mpp: state.camera.metres_per_pixel(target.height),
    })
}

fn want_overlay(state: &mut State, key: &str, wanted: overlay::Wanted) {
    let Some(overlay) = state.overlays.iter().find(|o| o.key == key) else { return };
    let Some(raster) = overlay.raster(wanted.choice.role) else { return };
    let Some(meta) = raster.meta.as_ref() else { return };
    let fingerprint = meta.fingerprint.clone();
    let label = overlay.label.clone();

    let owner = (key.to_string(), wanted.choice.role);
    let pointwise = meta.pointwise(wanted.want.level);
    if let Some(correlation) = state.passes.stale(&fingerprint, &owner, &wanted.want, pointwise) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    let going = state.passes.going(&fingerprint);
    let overlay = state.overlays.iter_mut().find(|o| o.key == key).expect("the overlay was just here");
    let raster = overlay.raster_mut(wanted.choice.role).expect("the raster was just here");
    if !going {
        raster.fetch.resume();
    }
    let Some(cells) = raster.fetch.ask(&state.tiles, &fingerprint, wanted.want.cells) else {
        return;
    };

    veldsdk::log::debug!(target: "handlers", "{}: {:?} level {}, {} cells",
        label, wanted.choice.role, wanted.choice.level, cells.len());

    let correlation = state.pending_query.begin(QueryCtx {
        key: key.to_string(),
        role: wanted.choice.role,
        fingerprint: fingerprint.clone(),
        cells: cells.clone(),
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level: wanted.choice.level,
        tiles: cells.iter().map(|&(_, x, y)| QueryAddr { x, y }).collect(),
        label,
    }, &correlation);
}

fn accept_tile(
    state: &mut State,
    key: &str,
    role: Role,
    fingerprint: &str,
    addr: Addr,
    texture: Option<veldsdk::proto::core::ResourceHandle>,
    width: u32,
    height: u32,
) {
    let landed = match &state.device {
        Some(device) => state.tiles.land(fingerprint, addr, texture, width, height, |view| {
            device.overlay_bind_group(view)
        }),
        None => {
            tiles::release(texture);
            tiles::Landing::Retry
        }
    };

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && let Some(raster) = overlay.raster_mut(role)
        && raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint)
    {
        match landed {
            tiles::Landing::Landed => raster.fetch.arrived(addr),
            tiles::Landing::Retry => {
                if raster.fetch.stumbled(addr) {
                    state.epoch += 1;
                }
            }
            tiles::Landing::Verdict => {
                raster.fetch.rejected(addr);
                state.epoch += 1;
            }
        }
    }
}

fn build_patches(state: &mut State) {
    let Some(look) = looking(state) else {
        state.ripple = None;
        report_progress(state, &[]);
        return;
    };

    let now = (look, state.tiles.generation, state.epoch);
    if state.checked == Some(now) {
        return;
    }
    state.checked = Some(now);

    let cap = cap_tiles(state);

    let (toll, began) = (perf::Toll::default(), Instant::now());
    let wanted: Vec<(String, f32, overlay::Wanted)> = state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .flat_map(|overlay| {
            overlay
                .wanted(&look, cap, &state.tiles, &toll)
                .into_iter()
                .map(|wanted| (overlay.key.clone(), overlay.opacity, wanted))
                .collect::<Vec<_>>()
        })
        .collect();
    state.perf.pass(perf::Pass::Patches, &toll, began.elapsed(), Instant::now());
    report_progress(state, &wanted);

    let mut pieces: Vec<Vec<overlay::Piece>> = Vec::with_capacity(wanted.len());
    {
        let State { overlays, tiles, .. } = &mut *state;
        for (key, _, wanted) in &wanted {
            let found = overlays.iter().find(|o| &o.key == key);
            pieces.push(found.map_or_else(Vec::new, |o| overlay::pieces(o, wanted, tiles)));
        }
    }

    state.ripple = wanted
        .iter()
        .zip(&pieces)
        .filter(|(_, pieces)| pieces.iter().any(overlay::Piece::coming))
        .filter_map(|((key, ..), _)| state.overlays.iter().find(|o| &o.key == key))
        .map(|overlay| ripple_strength(overlay.progress.within))
        .max_by(f32::total_cmp);

    let stamp: Vec<overlay::Built> = wanted
        .iter()
        .zip(&pieces)
        .filter_map(|((key, _, wanted), pieces)| {
            let drawn: Vec<overlay::Drawn> = pieces.iter().map(overlay::Piece::drawn).collect();
            Some(state.overlays.iter().find(|o| &o.key == key)?.built(wanted, &drawn))
        })
        .collect();
    if state.built.as_ref() == Some(&stamp) {
        return;
    }

    let mut draws = Vec::new();
    let assembled = Instant::now();
    let State { overlays, patch_vertices, .. } = state;
    patch_vertices.clear();
    for ((key, _, wanted), pieces) in wanted.iter().zip(&pieces) {
        let Some(overlay) = overlays.iter_mut().find(|o| &o.key == key) else { continue };
        overlay::patches(overlay, wanted, pieces, patch_vertices, &mut draws);
    }

    let filled = state.batch.fill(&state.patch_vertices, &draws);
    state.perf.rebuilt(state.patch_vertices.len(), assembled.elapsed());
    match filled {
        Ok(()) => {
            state.patches += 1;
            state.built = Some(stamp);
        }
        Err(error) => {
            veldsdk::log::error!(target: "render", "overlay patches not filled: {:#}", error);
        }
    }
}

const RIPPLE_PERIOD_S: f32 = 1.6;

const RIPPLE_STEPS: f32 = RIPPLE_PERIOD_S * 30.0;

fn advance_ripple(phase: f32, dt: f32, ripple: Option<f32>) -> (f32, u32) {
    let phase = match ripple {
        Some(_) => (phase + dt / RIPPLE_PERIOD_S).fract(),
        None => phase,
    };
    (phase, (phase * RIPPLE_STEPS) as u32)
}

fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

fn report_progress(state: &mut State, wanted: &[(String, f32, overlay::Wanted)]) {
    for overlay in &mut state.overlays {
        let Some((.., mine)) = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back()
        else {
            continue;
        };
        let Some(raster) = overlay.rasters.iter().find(|raster| raster.role == mine.choice.role)
        else {
            continue;
        };
        overlay.progress = overlay::progress_of(raster, &mine.want);
    }

    let overlays: Vec<OverlayProgress> = state
        .overlays
        .iter()
        .map(|overlay| {
            let mine = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back();
            overlay.report(mine.map(|(.., wanted)| wanted), &state.passes)
        })
        .chain(state.refused.iter().map(|(key, why)| OverlayProgress {
            key: key.clone(),
            ready: 0,
            total: 0,
            working: false,
            share: 0.0,
            error: why.clone(),
            trouble: String::new(),
            step: 0,
            steps: 0,
            blank: false,
            pass_read: 0,
            pass_total: 0,
            detailed: None,
            detailed_trouble: String::new(),
            detailed_variable: None,
            detailed_variables: Vec::new(),
        }))
        .collect();

    crate::emit::on_overlay_progress(&OverlaysProgress { overlays });
}

fn upload_outlines(state: &mut State) {
    let State { device: Some(device), outlines, .. } = state else { return };
    let built = outlines::Outlines::build(outlines);
    veldsdk::log::debug!(target: "render",
        "outlines: {} of them, {} line vertices, {} ribbon vertices, {} hatch vertices",
        outlines.len(), built.vertices.len(), built.ribbon.len(), built.hatch.len());
    if let Err(error) = device.set_outlines(&built) {
        veldsdk::log::error!(target: "render", "outlines not filled: {:#}", error);
    }
}

pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    let Some(app_proto::ui_event::Event::Frame(frame)) = event.event else {
        return;
    };
    if let Some(report) = state.perf.frame(Instant::now()) {
        veldsdk::log::debug!(target: "perf", "{}", report);
    }
    if state.camera.advance(frame.dt) {
        want_tiles(state, perf::Pass::Camera);
    }
    build_patches(state);

    let (phase, stepped) = advance_ripple(state.phase, frame.dt, state.ripple);
    state.phase = phase;

    let (Some(device), Some(target)) = (&state.device, &state.target) else { return };

    let now = Frame {
        camera: state.camera,
        texture: target.texture_id,
        generation: state.generation,
        patches: state.patches,
        ripple: stepped,
    };
    if state.drawn == Some(now) {
        return;
    }

    let ripple = gpu::Ripple { phase, strength: state.ripple.unwrap_or(0.0) };
    if let Err(error) = gpu::render(device, target, &state.camera, &state.batch, ripple) {
        veldsdk::log::error!(target: "render", "frame not written: {:#}", error);
        return;
    }
    state.drawn = Some(now);
}
