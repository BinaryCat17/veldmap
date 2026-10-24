pub mod camera;
pub mod gpu;
pub mod view;

use std::collections::{HashMap, HashSet};

use veldmap_image_tiler_wrap::tiles::{self, Addr, Missed, Passes, Store};
use veldsdk::proto::app as app_proto;

use crate::proto::image_tiler::{
    Described, DescribeRequest, ProduceDone, ProduceProgress, ProduceRequest,
    TileAddr, TileResult as ProducedTile,
};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, CloseView, ShowRequest, Variable, VariableRequest, ViewState,
};
use crate::proto::tile_cache::{
    QueryDone, QueryRequest, TileAddr as QueryAddr, TileResult as CachedTile,
};

use camera::Camera;
use gpu::Device;
use view::{Shown, Stamp, View};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
}

pub struct QueryCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct ProduceCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct State {
    device: Option<Device>,
    views: HashMap<String, View>,
    tiles: Store,
    pending_describe: veldsdk::Correlator<String>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    passes: Passes<String>,
    phase: f32,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        device: None,
        views: HashMap::new(),
        tiles: Store::new(config.vram_budget_mb * 1024 * 1024),
        pending_describe: veldsdk::Correlator::new(),
        pending_query: veldsdk::Correlator::new(),
        pending_produce: veldsdk::Correlator::new(),
        passes: Passes::default(),
        phase: 0.0,
    })
}

pub fn on_canvas(state: &mut State, msg: Canvas) {
    if msg.view.is_empty() {
        veldsdk::log::warn!(target: "handlers", "canvas without a view name");
        return;
    }
    let Some(surface) = msg.surface else { return };

    let view = state.views.entry(msg.view.clone()).or_insert_with(|| View::new(msg.view.clone()));
    let Some(texture) = surface.surface else {
        veldsdk::log::info!(target: "handlers", "{}: place recalled", view.label);
        view.target = None;
        return;
    };
    if surface.width == 0 || surface.height == 0 {
        veldsdk::log::warn!(target: "handlers", "{}: place {}x{} — nowhere to draw",
            view.label, surface.width, surface.height);
        return;
    }

    if state.device.as_ref().is_none_or(|device| device.format != surface.format) {
        match Device::create(surface.format) {
            Ok(device) => {
                state.tiles = Store::new_like(&state.tiles);
                state.device = Some(device);
            }
            Err(error) => {
                complain(state, &msg.view, format!("device resources not assembled: {:#}", error));
                return;
            }
        }
    }

    let view = state.views.get_mut(&msg.view).expect("the view was just there");
    match gpu::Target::create(texture.id, surface.width, surface.height) {
        Ok(target) => {
            view.target = Some(target);
            view.trouble = None;
        }
        Err(error) => {
            complain(state, &msg.view, format!("frame place not assembled: {:#}", error));
            return;
        }
    }

    fit_if_first(state, &msg.view);
    want_tiles(state, &msg.view);
    report(state, &msg.view);
}

fn complain(state: &mut State, key: &str, why: String) {
    veldsdk::log::error!(target: "handlers", "{}: {}", key, why);
    if let Some(view) = state.views.get_mut(key) {
        view.target = None;
        view.trouble = Some(why);
    }
    report(state, key);
}

pub fn on_show(state: &mut State, msg: ShowRequest) {
    let Some(resource) = msg.resource else {
        veldsdk::log::warn!(target: "handlers", "show without a resource");
        return;
    };
    if msg.view.is_empty() {
        veldsdk::log::warn!(target: "handlers", "show without a view name — the resource is being released");
        veldsdk::resource::release(resource);
        return;
    }

    let previous =
        state.views.get(&msg.view).and_then(|view| view.meta()).map(|meta| meta.fingerprint.clone());
    if let Some(fingerprint) = previous {
        release_pass(state, &fingerprint, &msg.view);
    }

    let view = state.views.entry(msg.view.clone()).or_insert_with(|| View::new(msg.view.clone()));
    if !msg.label.is_empty() {
        view.label = msg.label.clone();
    }
    view.variable = msg.variable.clone();

    view.fetch.reset();
    view.error = None;
    view.trouble = None;
    view.stuck = None;
    view.read_bytes = 0;
    view.total_bytes = resource.size;
    view.camera = None;
    view.drawn = None;
    view.shown = None;

    if let Err(error) = veldsdk::resource::grant_read_or_free(resource.id, "image-tiler") {
        view.error = Some(error);
        report(state, &msg.view);
        return;
    }
    view.shown = Some(Shown {
        resource: veldsdk::OwnedResource::new(resource.clone()),
        meta: None,
    });

    let correlation = view.describe.begin();
    state.pending_describe.insert(correlation.clone(), msg.view.clone());
    crate::calls::image_tiler::on_describe(&DescribeRequest {
        resource: Some(resource),
        label: view.label.clone(),
        geolocation: None,
        variable: view.variable.clone(),
    }, &correlation);

    report(state, &msg.view);
}

pub fn on_variable(state: &mut State, msg: VariableRequest) {
    let Some(view) = state.views.get(&msg.view) else { return };
    let Some(resource) = view.shown.as_ref().map(|shown| shown.resource.handle()) else { return };
    if view.variable == msg.variable {
        return;
    }
    if let Some(fingerprint) = view.meta().map(|meta| meta.fingerprint.clone()) {
        release_pass(state, &fingerprint, &msg.view);
    }

    let view = state.views.get_mut(&msg.view).expect("the view was found above");
    view.variable = msg.variable.clone();
    view.previous_plane = view.meta().map(|meta| (meta.width, meta.height));
    view.fetch.reset();
    view.error = None;
    view.trouble = None;
    view.stuck = None;
    view.drawn = None;
    if let Some(shown) = &mut view.shown {
        shown.meta = None;
    }
    let label = view.label.clone();
    let correlation = view.describe.begin();
    state.pending_describe.insert(correlation.clone(), msg.view.clone());
    crate::calls::image_tiler::on_describe(
        &DescribeRequest { resource: Some(resource), label, geolocation: None, variable: msg.variable },
        &correlation,
    );

    report(state, &msg.view);
}

pub fn on_close(state: &mut State, msg: CloseView) {
    let Some(view) = state.views.remove(&msg.view) else { return };
    if let Some(fingerprint) = view.meta().map(|meta| meta.fingerprint.clone()) {
        release_pass(state, &fingerprint, &msg.view);
    }
    veldsdk::log::debug!(target: "handlers", "{}: view closed", view.label);
}

pub fn on_camera(state: &mut State, msg: CameraCommand) {
    let Some(view) = state.views.get_mut(&msg.view) else { return };
    let (Some(target), Some(meta)) = (&view.target, view.shown.as_ref().and_then(|s| s.meta.as_ref()))
    else {
        return;
    };
    let img = (meta.width, meta.height);
    let canvas = (target.width, target.height);

    match msg.command {
        Some(Command::Fit(_)) => view.camera = Some(Camera::fit(img.0, img.1, canvas.0, canvas.1)),
        Some(Command::ZoomAt(zoom)) => {
            if let Some(camera) = &mut view.camera {
                camera.zoom_at(f64::from(zoom.x), f64::from(zoom.y), f64::from(zoom.factor), img, canvas);
            }
        }
        Some(Command::Pan(pan)) => {
            if let Some(camera) = &mut view.camera {
                camera.pan(f64::from(pan.dx), f64::from(pan.dy), img);
            }
        }
        None => {}
    }

    want_tiles(state, &msg.view);
    report(state, &msg.view);
}

pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some(key) = state.pending_describe.take(&correlation) else { return };
    let Some(view) = state.views.get_mut(&key) else { return };
    if view.describe.settle(&correlation) != veldsdk::Reply::Current {
        return;
    }

    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            view.error = Some(error);
            report(state, &key);
            return;
        }
    };

    veldsdk::log::info!(target: "handlers", "{}: {}", view.label, meta.note());

    if view.plane_changed(&meta) {
        view.camera = None;
    }
    if let Some(shown) = &mut view.shown {
        shown.meta = Some(meta);
    }

    fit_if_first(state, &key);
    want_tiles(state, &key);
    report(state, &key);
}

pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let cap = cap_tiles(state);
    let Some(view) = state.views.get_mut(&ctx.view) else { return };

    if view.meta().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
        return;
    }

    let error = veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone());
    if !error.is_empty() {
        view.fetch.forget_asked(&ctx.cells);
        view.trouble = Some(format!("incomplete: {}", error));
        report(state, &ctx.view);
        return;
    }

    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);
    let desired: HashSet<Addr> = view::wanted(view, &state.tiles, cap)
        .map(|want| want.cells.into_iter().collect())
        .unwrap_or_default();
    let pointwise = view.meta().is_some_and(|meta| meta.pointwise(level));
    let missed = view.fetch.missed(
        &state.passes,
        &ctx.fingerprint,
        &ctx.view,
        pointwise,
        &ctx.cells,
        msg.misses.iter().map(|addr| (level, addr.x, addr.y)),
        |addr| desired.contains(&addr),
    );
    let (handle, label) = match view.shown.as_ref() {
        Some(shown) => (shown.resource.handle(), view.label.clone()),
        None => return,
    };

    let produce_list = match missed {
        Missed::Produce(cells) => cells,
        Missed::Waiting => {
            report(state, &ctx.view);
            return;
        }
        Missed::Closed => {
            want_tiles(state, &ctx.view);
            report(state, &ctx.view);
            return;
        }
    };

    let correlation = state.pending_produce.begin(ProduceCtx {
        view: ctx.view.clone(),
        fingerprint: ctx.fingerprint.clone(),
        cells: produce_list.clone(),
    });
    state.passes.begin(
        &ctx.fingerprint,
        ctx.view.clone(),
        correlation.clone(),
        level,
        produce_list.clone(),
    );
    let variable = state.views.get(&ctx.view).map(|view| view.variable.clone()).unwrap_or_default();
    crate::calls::image_tiler::on_produce(&ProduceRequest {
        resource: Some(handle),
        level,
        tiles: produce_list.iter().map(|&(_, x, y)| TileAddr { x, y }).collect(),
        label,
        variable,
    }, &correlation);

    report(state, &ctx.view);
}

pub fn on_produce_progress(state: &mut State, msg: ProduceProgress) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else { return };
    let key = ctx.view.clone();
    if let Some(view) = state.views.get_mut(&key) {
        view.read_bytes = msg.read_bytes;
        view.total_bytes = msg.total_bytes;
        report(state, &key);
    }
}

pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };

    let ours_kill = state.passes.finish(&correlation);

    if let Some(view) = state.views.get_mut(&ctx.view) {
        let ours = view.meta().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
        let error = match ours_kill {
            true => msg.error.clone(),
            false => veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone()),
        };
        let failed = ours && !error.is_empty();
        view.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
        if ours {
            (view.read_bytes, view.total_bytes) = (0, 0);
        }
        if failed {
            veldsdk::log::warn!(target: "handlers", "{}: production: {}", view.label, error);
            view.trouble = Some(format!("incomplete: {}", error));
        }
    }

    for key in watchers(state, &ctx.fingerprint) {
        want_tiles(state, &key);
        report(state, &key);
    }
}

fn watchers(state: &State, fingerprint: &str) -> Vec<String> {
    state
        .views
        .iter()
        .filter(|(_, view)| view.meta().is_some_and(|meta| meta.fingerprint == fingerprint))
        .map(|(key, _)| key.clone())
        .collect()
}

fn release_pass(state: &mut State, fingerprint: &str, view: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &view.to_string()) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

const RIPPLE_STEPS: f32 = view::RIPPLE_PERIOD_S * 30.0;

fn advance_ripple(phase: f32, dt: f32, glowing: bool) -> (f32, u32) {
    let phase = match glowing {
        true => (phase + dt / view::RIPPLE_PERIOD_S).fract(),
        false => phase,
    };
    (phase, (phase * RIPPLE_STEPS) as u32)
}

pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    let Some(app_proto::ui_event::Event::Frame(frame)) = event.event else {
        return;
    };
    let cap = cap_tiles(state);
    let glowing = state.views.values().any(|view| view.glowing);
    let (phase, stepped) = advance_ripple(state.phase, frame.dt, glowing);
    state.phase = phase;
    let mut complained: Vec<String> = Vec::new();
    {
    let State { views, tiles, device: Some(device), .. } = state else { return };

    for (key, view) in views.iter_mut() {
        let Some(camera) = view.camera else { continue };
        let Some(target) = &view.target else { continue };
        let stamp = Stamp {
            camera,
            generation: tiles.generation,
            texture: target.texture_id,
            ripple: stepped,
        };
        if view.drawn == Some(stamp) {
            continue;
        }

        let quads = view::quads(view, tiles, cap, phase);
        view.glowing = quads.iter().any(|quad| quad.glow > 0.0);
        let target = view.target.as_ref().expect("the place is checked above");
        let turn = view.turn;
        view.turn ^= 1;
        match gpu::render(device, target, &mut view.vertices[turn], &quads) {
            Ok(()) => {
                view.drawn = Some(stamp);
                if view.stuck.take().is_some() {
                    complained.push(key.clone());
                }
            }
            Err(error) => {
                veldsdk::log::error!(target: "render", "{}: frame not written: {:#}", view.label, error);
                view.drawn = Some(stamp);
                view.stuck = Some(format!("frame stuck: {}", error));
                complained.push(key.clone());
            }
        }
    }
    }
    for key in complained {
        report(state, &key);
    }
}

fn fit_if_first(state: &mut State, key: &str) {
    let Some(view) = state.views.get_mut(key) else { return };
    if view.camera.is_some() {
        return;
    }
    let (Some(target), Some(meta)) = (&view.target, view.shown.as_ref().and_then(|s| s.meta.as_ref()))
    else {
        return;
    };
    view.camera = Some(Camera::fit(meta.width, meta.height, target.width, target.height));
}

fn want_tiles(state: &mut State, key: &str) {
    let cap = cap_tiles(state);
    let Some(view) = state.views.get(key) else { return };
    let Some(want) = view::wanted(view, &state.tiles, cap) else { return };
    let Some((fingerprint, pointwise)) = view
        .meta()
        .map(|meta| (meta.fingerprint.clone(), meta.pointwise(want.level)))
    else {
        return;
    };
    let label = view.label.clone();

    if let Some(correlation) = state.passes.stale(&fingerprint, &key.to_string(), &want, pointwise)
    {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
    let going = state.passes.going(&fingerprint);
    let view = state.views.get_mut(key).expect("the view was just there");
    if !going {
        view.fetch.resume();
    }
    let Some(cells) = view.fetch.ask(&state.tiles, &fingerprint, want.cells) else { return };

    veldsdk::log::debug!(target: "handlers", "{}: step {} ({} of {}), {} cells",
        label, want.level, want.climbed + 1, want.steps, cells.len());

    let correlation = state.pending_query.begin(QueryCtx {
        view: key.to_string(),
        fingerprint: fingerprint.clone(),
        cells: cells.clone(),
    });
    crate::calls::tile_cache::on_query(&QueryRequest {
        fingerprint,
        level: want.level,
        tiles: cells.iter().map(|&(_, x, y)| QueryAddr { x, y }).collect(),
        label,
    }, &correlation);
}

fn cap_tiles(state: &State) -> u64 {
    state.tiles.cap_tiles(
        state
            .views
            .values()
            .filter(|view| view.target.is_some() && view.camera.is_some())
            .filter_map(|view| view.meta().map(|meta| meta.fingerprint.as_str())),
    )
}

fn accept_tile(
    state: &mut State,
    view_key: &str,
    fingerprint: &str,
    addr: Addr,
    texture: Option<veldsdk::proto::core::ResourceHandle>,
    width: u32,
    height: u32,
) {
    let landed = match &state.device {
        Some(device) => state.tiles.land(fingerprint, addr, texture, width, height, |view| {
            device.tile_bind_group(view)
        }),
        None => {
            tiles::release(texture);
            tiles::Landing::Retry
        }
    };

    if let Some(view) = state.views.get_mut(view_key)
        && view.meta().is_some_and(|meta| meta.fingerprint == fingerprint)
    {
        match landed {
            tiles::Landing::Landed => {
                view.fetch.arrived(addr);
                view.landed();
            }
            tiles::Landing::Retry => {
                view.fetch.stumbled(addr);
            }
            tiles::Landing::Verdict => view.fetch.rejected(addr),
        }
    }
}

fn report(state: &State, key: &str) {
    let Some(view) = state.views.get(key) else { return };
    let want = view::wanted(view, &state.tiles, cap_tiles(state));
    let current = view_state(view, key, want.as_ref(), &state.passes);
    crate::emit::on_view_state(&current);
}

fn wire_variable(variable: &tiles::Variable) -> Variable {
    Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

fn view_state(
    view: &View,
    key: &str,
    want: Option<&tiles::Want>,
    passes: &Passes<String>,
) -> ViewState {
    let meta = view.shown.as_ref().and_then(|shown| shown.meta.as_ref());
    let ordered = view.fetch.ordered();
    let inside = tiles::inside(
        ordered,
        (view.read_bytes, view.total_bytes),
        meta.zip(want).is_some_and(|(meta, want)| meta.pointwise(want.level)),
    );
    let working = view.working(passes, want);
    ViewState {
        view: key.to_string(),
        source_width: meta.map_or(0, |meta| meta.width),
        source_height: meta.map_or(0, |meta| meta.height),
        scale: view.camera.map_or(0.0, |camera| camera.scale),
        read_bytes: inside.read.0,
        total_bytes: inside.read.1,
        ready: ordered.0,
        total: ordered.1,
        step: want.map_or(0, |want| want.climbed),
        steps: want.map_or(0, |want| want.steps),
        working,
        error: view.error.clone().unwrap_or_default(),
        trouble: view.said(want.is_some() && !working),
        needs_place: view.target.is_none() && view.shown.is_some() && view.error.is_none(),
        variable: meta.and_then(|meta| meta.variable.as_ref()).map(wire_variable),
        variables: meta.map(|meta| meta.variables.iter().map(wire_variable).collect()).unwrap_or_default(),
    }
}
