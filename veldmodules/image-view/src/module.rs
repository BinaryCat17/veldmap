//! image-view: canvases for viewing rasters on top of the tile pipeline.
//!
//! The layout owner sends the place (on_canvas), the resource (on_show,
//! ownership moves here) and camera intents; only on_view_state goes back.
//! Tiles are asked of tile-cache, misses are ordered from image-tiler; its
//! read grant on the resource is issued here, at show time.
//!
//! There is one pass over the source: for a format without random access each
//! produce is a full pass over the file, and running a second one alongside
//! the first would read gigabytes twice. While it runs, misses on this source
//! do not go to production — neither its own nor those of a neighboring tab
//! with the same file; once it finishes, what is needed is recomputed for
//! everyone watching it, and almost everything is found in the cache: the
//! pass stores all levels there. A pass is killed in two ways, both by its own
//! customer: it leaves with the snapshot (`release_pass`), or it needs a level
//! coarser than its own pass produces (`Passes::stale`).
//!
//! module.rs holds the state and handlers; the camera is in camera.rs, the
//! frame in view.rs and gpu.rs. Tiles in video memory and the accounting of
//! what was asked are shared with the globe (`veldmap_image_tiler_wrap::tiles`).

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
    /// Video memory budget for tiles, MiB. Eviction is forgetting: the tile
    /// stays on disk at tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
}

/// Whose cache reply this is and what was asked. The fingerprint is its own,
/// not the view's: the show could change while the reply was in flight, and
/// its tiles must not be filed under the new fingerprint.
pub struct QueryCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

/// The same for the producer.
pub struct ProduceCtx {
    view: String,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct State {
    /// Appears at the first place for a canvas: before it there is neither a
    /// target format nor a reason to build anything.
    device: Option<Device>,
    views: HashMap<String, View>,
    tiles: Store,
    pending_describe: veldsdk::Correlator<String>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    /// Running producer passes, one per source. Not on the view: a pass reads
    /// the file, and the file is the same for two tabs with the same snapshot.
    /// The customer here is the tab name: only the tab that started a pass may
    /// take it off the books.
    passes: Passes<String>,
    /// Where the ripple wave stands, 0..1 of a period. One for all canvases:
    /// the ripple is about ongoing work, not about a view.
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

// ── Layout owner inputs ─────────────────────────────────────────

/// Canvas place. An empty surface is a recall: the view lives, but there is
/// nowhere to draw.
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
                // Tile bind groups are assembled for the previous device's
                // layout — they are incompatible with a new one. The store is
                // emptied: tiles lie on disk and return in milliseconds.
                state.tiles = Store::new_like(&state.tiles);
                state.device = Some(device);
            }
            Err(error) => {
                // The reason goes to the screen, not just to the log: a silent
                // empty tab reads as "frozen", not as a failure.
                complain(state, &msg.view, format!("device resources not assembled: {:#}", error));
                return;
            }
        }
    }

    let view = state.views.get_mut(&msg.view).expect("the view was just there");
    match gpu::Target::create(texture.id, surface.width, surface.height) {
        Ok(target) => {
            view.target = Some(target);
            // The place is assembled — the old complaint about it no longer
            // belongs to this frame.
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

/// The frame place could not be assembled: the reason goes to both the log
/// and the screen — but as a complaint, not as a verdict.
///
/// The difference is not cosmetic. The client reads a verdict
/// (`ViewState.error`) as "nothing to watch" and **removes the canvas from the
/// layout**; and without a canvas the next delegation of a place never comes
/// to us — that is, the tab stays dead until closed, even when the failure was
/// momentary (the texture was swapped while the event was on its way). A
/// complaint keeps the canvas in place.
///
/// By itself this still does not bring the place back: the layout owner
/// allocates it, and it is only asked for by a resize, which did not happen
/// here. So the need is stated by a separate field — `ViewState.needs_place` —
/// and on it the owner issues the place again (see `report`).
///
/// The verdict remains for what delegation cannot cure: the source failed to
/// open or to describe — there truly is nothing to watch there.
fn complain(state: &mut State, key: &str, why: String) {
    veldsdk::log::error!(target: "handlers", "{}: {}", key, why);
    if let Some(view) = state.views.get_mut(key) {
        view.target = None;
        view.trouble = Some(why);
    }
    report(state, key);
}

/// Show a snapshot. The resource is already ours: the sender transferred
/// ownership.
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

    // The previous show ends here, and its pass leaves with it: it reads the
    // resource that is about to be released. In-flight tails are recognized by
    // fingerprint and dropped.
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
    // And the complaint about a stuck frame: it is about the previous
    // snapshot, while a different one is already shown — the view in this tab
    // is reused, not created anew.
    view.stuck = None;
    view.read_bytes = 0;
    view.total_bytes = resource.size;
    view.camera = None;
    view.drawn = None;
    view.shown = None;

    // The grant comes before ownership: on refusal the helper releases the
    // resource itself, and wrapping it into an owner would be a second release.
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
        // The canvas needs no binding at all: it shows the raster as a
        // picture, not laid onto the Earth.
        geolocation: None,
        variable: view.variable.clone(),
    }, &correlation);

    report(state, &msg.view);
}

/// Another variable of the same file: a fresh describe with it, the resource
/// unchanged. The previous pass leaves with its previous fingerprint — its
/// tiles are no longer about this show — while the camera stays as long as the
/// plane is the same: refitting would jerk the frame under the hand; a
/// different plane is answered by the describe (`View::plane_changed`).
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

/// End of a view: everything of it goes away. Tiles in the store are shared
/// and stay until evicted: the same tab, reopened, will start from them.
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

// ── Tile pipeline replies ───────────────────────────────────────

pub fn on_described(state: &mut State, msg: Described) {
    let correlation = veldsdk::correlation();
    let Some(key) = state.pending_describe.take(&correlation) else { return };
    let Some(view) = state.views.get_mut(&key) else { return };
    if view.describe.settle(&correlation) != veldsdk::Reply::Current {
        // The show changed while this was in flight — and that describe's
        // resource is already released.
        return;
    }

    // A shared rule decides whether a describe is fit: there is one tiler and
    // one pyramid, so there is nothing to disagree about with the overlay on.
    // What is local here is only what to do with an unfit one: the canvas has
    // a place on screen, and the reason goes there.
    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            view.error = Some(error);
            report(state, &key);
            return;
        }
    };

    veldsdk::log::info!(target: "handlers", "{}: {}", view.label, meta.note());

    // Another variable of another plane — the old camera would fit it
    // off-target.
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

/// A tile from the disk cache.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// A tile from the producer.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (view_key, fingerprint) = (ctx.view.clone(), ctx.fingerprint.clone());
    accept_tile(state, &view_key, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// The cache answered with all it could; misses go to the producer.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let cap = cap_tiles(state);
    let Some(view) = state.views.get_mut(&ctx.view) else { return };

    // The show changed while the reply was in flight: this request's
    // expectations are no longer ours.
    if view.meta().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
        return;
    }

    // The cache refused — but the snapshot is not spoiled by it: expectations
    // are dropped and the cells will be asked again at the next recompute.
    // Showing the reason instead of the frame would erase the picture over a
    // hiccup that passes on its own. A reply negotiated by the host is the
    // same refusal, and its path is the same: drop one's own cells from
    // expectations and say it out loud. Taken for a success, it would leave
    // them hanging forever (see `veldsdk::reply::undelivered`).
    let error = veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone());
    if !error.is_empty() {
        view.fetch.forget_asked(&ctx.cells);
        view.trouble = Some(format!("incomplete: {}", error));
        report(state, &ctx.view);
        return;
    }

    // Misses that are still visible go to production; those that left the
    // screen are dropped from expectations and re-asked when they return to
    // the frame.
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);
    let desired: HashSet<Addr> = view::wanted(view, &state.tiles, cap)
        .map(|want| want.cells.into_iter().collect())
        .unwrap_or_default();
    // Whether the requested level is read pointwise: it decides whether to
    // wait for one's own pass to finish silently or to re-ask the cache.
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
    // The source handle and the label are taken right here: further on
    // `state` is busy with accounting, and a reference to it cannot be held
    // along with a view.
    let (handle, label) = match view.shown.as_ref() {
        Some(shown) => (shown.resource.handle(), view.label.clone()),
        None => return,
    };

    let produce_list = match missed {
        Missed::Produce(cells) => cells,
        // Wait for a foreign pass silently: its end will recompute what is
        // needed for everyone watching this source.
        Missed::Waiting => {
            report(state, &ctx.view);
            return;
        }
        // The cache closed the order entirely — the step is passed, and the
        // next one must be asked for right here (see `Missed::Closed`).
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

/// The only end of production, whatever its kind — including for a killed one
/// the host answers with an empty message.
pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };

    // The guard is removed first of all and unconditionally: it stood on the
    // source, not on the view, and the pass's customer could have been closed
    // while the pass ran. Leaving here without removing it, neighboring tabs
    // with the same file would wait for an end that will never come.
    // A pass we took down ourselves is a normal step of refinement, not a
    // failure (see `tiles::Passes::finish`).
    let ours_kill = state.passes.finish(&correlation);

    if let Some(view) = state.views.get_mut(&ctx.view) {
        // The show could change while the pass ran: then its cells are no
        // longer about this view, and neither expectations nor refusals apply
        // to it.
        let ours = view.meta().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
        // An empty reply negotiated by the host for a crashed one is a failed
        // pass, not a successful one: taken for a success it would also
        // forgive cells that nobody produced (`Fetch::forgive`).
        let error = match ours_kill {
            true => msg.error.clone(),
            false => veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone()),
        };
        let failed = ours && !error.is_empty();
        view.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
        if ours {
            // What was read belongs to the finished pass and must not outlive
            // it: nothing is read between steps, and the status bar would show
            // "reading... 512 MB of 512 MB" the whole time.
            (view.read_bytes, view.total_bytes) = (0, 0);
        }
        if failed {
            // Do not re-ask what has already failed to produce: every camera
            // move would hammer the producer with the same refusal.
            //
            // But there is still something to watch: a failed pass is cells of
            // one step that did not arrive, not a bad snapshot. The steps
            // above are already drawn, and replacing them with the reason
            // would erase a finished picture over a single aborted read.
            veldsdk::log::warn!(target: "handlers", "{}: production: {}", view.label, error);
            view.trouble = Some(format!("incomplete: {}", error));
        }
    }

    // While the pass ran, requests were deferred — recompute what is needed
    // now, and not only for the customer: a neighboring tab with the same file
    // waited silently all this time, and no one else is left to wake it.
    for key in watchers(state, &ctx.fingerprint) {
        want_tiles(state, &key);
        report(state, &key);
    }
}

/// Tabs watching this source.
fn watchers(state: &State, fingerprint: &str) -> Vec<String> {
    state
        .views
        .iter()
        .filter(|(_, view)| view.meta().is_some_and(|meta| meta.fingerprint == fingerprint))
        .map(|(key, _)| key.clone())
        .collect()
}

/// A tab leaves a snapshot — or closes entirely: its pass is carried off with
/// it.
///
/// Carried off unconditionally, even when a neighboring tab watches the same
/// file: what a pass reads is not "the file" but that very resource which is
/// about to be released together with the show (see `tiles::Passes`). The
/// neighbor will start its own after this one ends — with its own resource,
/// which is alive.
fn release_pass(state: &mut State, fingerprint: &str, view: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &view.to_string()) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

// ── Frame tick ──────────────────────────────────────────────────

/// Into how many fractions of a period the ripple phase is quantized in the
/// frame stamp. Thirty steps a second is the limit past which the eye stops
/// discerning wave motion, and frames beyond it would be redraws for the same
/// picture.
const RIPPLE_STEPS: f32 = view::RIPPLE_PERIOD_S * 30.0;

/// Where the phase went over this tick and how the frame differs from the
/// previous one.
///
/// A pure function, because it guards against the main danger of an edit — a
/// canvas that draws forever: while there is nothing to ripple, the quantized
/// phase must stand still, otherwise the frame comparison stops letting rest
/// through.
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
    // The phase is one for all canvases: the ripple is about ongoing work, not
    // about a view, and diverging across views it would read as different
    // speeds of one job. The views that have something to ripple move it.
    let glowing = state.views.values().any(|view| view.glowing);
    let (phase, stepped) = advance_ripple(state.phase, frame.dt, glowing);
    state.phase = phase;
    // Views whose frame was not written: such a complaint must also be told,
    // and the state broadcast touches `state` as a whole — so after the loop.
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
        // What ripples in the next frame follows from the same quads drawn in
        // this one: nothing can answer it before the assembly.
        view.glowing = quads.iter().any(|quad| quad.glow > 0.0);
        let target = view.target.as_ref().expect("the place is checked above");
        // The vertex buffer is always a different one than in the previous
        // frame (see `View::vertices`).
        let turn = view.turn;
        view.turn ^= 1;
        match gpu::render(device, target, &mut view.vertices[turn], &quads) {
            // Only one's OWN complaint is cleared — the one about a stuck
            // frame. A successful frame does not touch a foreign one ("the
            // production failed"): it holds until a tile arrives (see
            // `View::landed`), and redrawing the same hole as a hole leaves it
            // in place.
            Ok(()) => {
                view.drawn = Some(stamp);
                if view.stuck.take().is_some() {
                    complained.push(key.clone());
                }
            }
            Err(error) => {
                veldsdk::log::error!(target: "render", "{}: frame not written: {:#}", view.label, error);
                // Exactly `trouble`, not `error`: there is still something to
                // watch, while the frame has frozen. And it is marked as
                // drawn: without that the attempt repeats on every frame tick,
                // i.e. writes to the log sixty times a second and burns a
                // frame for nothing, with the same reason every time.
                view.drawn = Some(stamp);
                view.stuck = Some(format!("frame stuck: {}", error));
                complained.push(key.clone());
            }
        }
    }
    }
    // A complaint left untold is a silent canvas: a frame tick makes no
    // broadcast, and a frozen view has no other events.
    for key in complained {
        report(state, &key);
    }
}

// ── Internals ──────────────────────────────────────────────────

/// First fit: the camera appears when both the snapshot and the place are
/// known for the first time. After that commands keep it alive.
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

/// Recompute of what is needed: visible cells minus the present, expected and
/// failed ones — into a cache request. One's own pass that produces a level
/// finer than needed is killed right here: it is no longer about what is being
/// watched.
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
    // No pass — cells deferred for its time have nothing left to wait for.
    if !going {
        view.fetch.resume();
    }
    let Some(cells) = view.fetch.ask(&state.tiles, &fingerprint, want.cells) else { return };

    // The fetch progress is visible only here: which step was needed, how many
    // of its cells are missing and how far sharpness still is (the same line
    // as for overlays on the globe).
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

/// The ceiling on one pyramid's appetite. The rule is shared with the globe
/// and lives at the budget (`Store::cap_tiles`); here only the answer to "how
/// many pyramids are being drawn now".
///
/// Pyramids, not tabs: two tabs with the same snapshot are one tile set and
/// one share of the budget.
fn cap_tiles(state: &State) -> u64 {
    state.tiles.cap_tiles(
        state
            .views
            .values()
            .filter(|view| view.target.is_some() && view.camera.is_some())
            .filter_map(|view| view.meta().map(|meta| meta.fingerprint.as_str())),
    )
}

/// A tile has arrived — into the store and out of the expectations.
///
/// The cell leaves the expectations either way: the answer about it arrived;
/// an answer without a landed tile is a miss, and it will be re-asked at the
/// next recompute. Left in the expectations, it would never be re-asked.
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
        // No device means there was never a canvas: once created, it survives
        // even a place recall (`on_canvas` clears only `target`). Tiles are not
        // asked before the first canvas, so this branch is for a reply that
        // arrived before the canvas was created.
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
            // Our hiccup: the cell will be asked again. A second time in a
            // row on the same cell, `stumbled` will condemn it by itself.
            tiles::Landing::Retry => {
                view.fetch.stumbled(addr);
            }
            tiles::Landing::Verdict => view.fetch.rejected(addr),
        }
    }
}

/// Broadcast of the show state. Called on every meaningful change, and only
/// what changed leaves: the topic is declared as a snapshot, and a repeat is
/// cut off by its stub (see schema.yaml).
fn report(state: &State, key: &str) {
    let Some(view) = state.views.get(key) else { return };
    // The desired is recomputed rather than taken from the last time: "work is
    // going" rests in particular on the path to the goal not yet traveled, and
    // that path is measured by what is needed now. Computing is cheap — for a
    // canvas the visible is the camera rectangle, with no projections.
    let want = view::wanted(view, &state.tiles, cap_tiles(state));
    let current = view_state(view, key, want.as_ref(), &state.passes);
    crate::emit::on_view_state(&current);
}

/// A variable onto the wire — as is.
fn wire_variable(variable: &tiles::Variable) -> Variable {
    Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

/// How the show looks from the outside — apart from the broadcast, because
/// only this shape is observable. Publishing in the native build is a stub
/// (`veldsdk::abi`): a message sent there goes unseen, while the returned
/// value is visible to the caller.
fn view_state(
    view: &View,
    key: &str,
    want: Option<&tiles::Want>,
    passes: &Passes<String>,
) -> ViewState {
    let meta = view.shown.as_ref().and_then(|shown| shown.meta.as_ref());
    let ordered = view.fetch.ordered();
    // Whether the bytes are fit is decided not by the show but by which path
    // the pass takes — a rule shared with the globe (`tiles::pointwise`): for
    // pointwise reading the counter is a high-water mark of the head, while
    // for one read whole by the pass it equals the size already before the
    // pass. Handed over as-is, they would write "24 of 24 MB" for the whole
    // minute of decoding. The level is the one the fetch runs at.
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
        // The step comes from the same ladder by which tiles are ordered, and
        // the cell count from the last completed order: counted separately,
        // they would tell of two different jobs. No ladder — nothing to count
        // by, and the zeros mean exactly that.
        ready: ordered.0,
        total: ordered.1,
        step: want.map_or(0, |want| want.climbed),
        steps: want.map_or(0, |want| want.steps),
        working,
        error: view.error.clone().unwrap_or_default(),
        // One complaint on the wire, three reasons, and `View::said` folds
        // them. They arrive as already-spoken words: there is no one to
        // re-label them, and the client does not know the difference (see
        // `View::stuck`). The fetch progress goes there for the same reason:
        // the client shows the complaint instead of it.
        trouble: view.said(want.is_some() && !working),
        // The place was not assembled — so only the layout owner can issue it
        // again: we do not allocate it ourselves (see `complain`). A condemned
        // view needs no place: there is no canvas for it in the layout anyway,
        // and a texture for it would be allocated in vain.
        needs_place: view.target.is_none() && view.shown.is_some() && view.error.is_none(),
        variable: meta.and_then(|meta| meta.variable.as_ref()).map(wire_variable),
        variables: meta.map(|meta| meta.variables.iter().map(wire_variable).collect()).unwrap_or_default(),
    }
}
