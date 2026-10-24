//! Three-dimensional view of the Earth.
//!
//! The module knows about the Earth and the camera and nothing else: not about tabs,
//! not about the cursor, not about what else is on screen at this moment. It gets its
//! space ready-made (`on_set_surface`) — just as ui-service gets the window surface —
//! and camera movement arrives already as an intent (`on_camera`), because only the one
//! who draws the interface can decide which gesture calls it.
//!
//! Coordinates are real: WGS84 and ECEF, the same ellipsoid and the same axes in which
//! latitude and longitude arrive from outside (see `geodesy`).

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
    /// Video memory budget for overlay tiles, in MiB. Eviction is forgetting:
    /// the tile stays on disk at tile-cache.
    #[serde(default = "default_vram_budget_mb")]
    pub vram_budget_mb: u64,
}

fn default_vram_budget_mb() -> u64 {
    tiles::DEFAULT_VRAM_BUDGET_MB
}

/// Whose cache reply this is and what was asked about. The fingerprint is our own,
/// not taken from the raster: the overlay could have been replaced while the reply
/// was on the way, and its tiles must not be placed under the new fingerprint.
pub struct QueryCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

/// The same for the producer.
pub struct ProduceCtx {
    key: String,
    role: Role,
    fingerprint: String,
    cells: Vec<Addr>,
}

pub struct State {
    camera: Camera,
    /// Appears on the first delegation of space: before it there is neither a
    /// target format to build pipelines for, nor a reason to allocate anything.
    device: Option<Device>,
    target: Option<Target>,
    /// The camera and texture in which the last frame is recorded. When they match
    /// the current ones, there is no reason to redraw: the Earth does not move by
    /// itself, and while nothing moves it, the target already holds exactly what
    /// would be drawn again. A tab taken off screen idles the same way: no camera
    /// events arrive from it, and there is nowhere for us to learn that it is not
    /// being shown.
    ///
    /// A comparison, not a "time to redraw" flag: a flag would have to be set in
    /// every place where something changes, and one day it would not be set. A new
    /// texture is enough for this comparison to tell them apart: resource ids are
    /// monotonic and never reused (see `ResourceRegistry::register`).
    drawn: Option<Frame>,
    /// Where the ripple wave stands, 0..1 of a period.
    phase: f32,
    /// Ripple strength when the frame has something to ripple; `None` — nothing to
    /// ripple, and the phase stands still.
    ///
    /// Computed by rebuilding the patches — over the same cells that are drawn —
    /// and kept in a field because the frame tick asks for it before assembling the
    /// frame fingerprint.
    ripple: Option<f32>,
    /// The last set of outlines sent. Stored because it can arrive before the space
    /// for rendering, and it can only be poured into buffers once the device is
    /// ready.
    outlines: Vec<crate::proto::globe::Outline>,
    /// How many times the outline set has changed. The outlines themselves cannot
    /// be compared in [`Frame`] without keeping a copy solely for comparison — and
    /// the counter answers the same question: is the set the same as in the last
    /// frame or already different.
    generation: u64,

    /// Overlays in arrival order — which is also the drawing order.
    overlays: Vec<Overlay>,
    /// Overlays we did not accept, and why: no binding, not a single raster opened.
    /// They did not become our overlays and there is nowhere for them to lie, but
    /// they must be reported — the sender considers them lying on the globe, and
    /// silence would leave him with "preparing…" forever.
    ///
    /// They live until the next set: on such a notice the sender removes the layer,
    /// and the set arrives again because of that.
    refused: Vec<(String, String)>,
    /// Overlay tile textures, shared by all overlays, with a budget.
    tiles: Store,
    /// Producer passes in flight, one per source. Not per raster: a pass belongs to
    /// the file, and there can be several rasters with one file. The owner here is
    /// the layer and the raster role: only the one who started a pass may take it
    /// off the books.
    passes: Passes<(String, Role)>,
    /// Assembled warp patches: a vertex buffer, index blocks by density, and draws.
    batch: gpu::OverlayBatch,
    /// Patch vertices that survived the last rebuild. Kept here for capacity, not
    /// for content: the composition changes on every camera movement, and for a
    /// global raster this is megabytes (hundreds of cells, each with nodes by the
    /// density of its warp mesh, sized like [`gpu::OverlayVertex`]) that a freshly
    /// created vector would grow by reallocation every time.
    ///
    /// Capacity is taken for the worst scene and never given back. There is no one
    /// to return it to: the wasm allocator does not return linear memory to the
    /// host, so a freed vector would free space only for itself.
    patch_vertices: Vec<gpu::OverlayVertex>,
    /// What the current patches are made of: raster choices, the tiles that covered
    /// them, and layer opacity (baked into the vertices). Diverged from the current
    /// state — rebuild (see [`build_patches`]).
    built: Option<Vec<overlay::Built>>,
    /// How many times the patches were rebuilt — their contribution to the frame
    /// comparison.
    patches: u64,
    /// Changes in overlay composition and fetching progress: acceptance, describing,
    /// removal, end of pass. Together with the camera and the store generation it
    /// answers the question "could the choice or the progress have changed": while
    /// all three are unchanged, the frame tick recomputes neither.
    epoch: u64,
    /// What the choices were checked against last time.
    ///
    /// The whole look, not the camera: it is computed from both the camera and the
    /// space for the frame (`looking`), so a change of area size changes the level
    /// choice no less than camera movement. Camera alone would not let these gates
    /// notice that.
    checked: Option<(overlay::Look, u64, u64)>,
    pending_describe: veldsdk::Correlator<(String, Role)>,
    pending_query: veldsdk::Correlator<QueryCtx>,
    pending_produce: veldsdk::Correlator<ProduceCtx>,
    /// What recomputing the desired costs: how long the grid walk takes and how
    /// many cells it checked in vain. This is measured only here.
    perf: perf::Meter,
}

/// What a frame is made of. Matches the current one — there is nothing to draw.
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    camera: Camera,
    texture: u64,
    generation: u64,
    patches: u64,
    /// Ripple phase, rounded down to the display step. While there is nothing to
    /// ripple, it stands still and the frame is skipped exactly as before it; as
    /// soon as a moving cell appears in the frame, it advances on every tick — and
    /// thereby turns on redraw by itself for the whole loading time. A separate
    /// "redraw" flag is therefore not needed: the frame comparison is that switch.
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

/// Space for the view: the owner allocated a texture and granted us the right to
/// write. An empty surface is a revocation: the space ended together with its owner.
///
/// The device stays on revocation: geometry and pipelines do not depend on the
/// space, and the globe tab is usually closed and opened again.
///
/// Pipelines are built for the target format, so a format change rebuilds them. In
/// practice it never changes, but that cannot be learned from here — and a mismatch
/// would surface as a render failure without a clear reason.
pub fn on_set_surface(state: &mut State, req: SurfaceDelegated) {
    let Some(surface) = req.surface else {
        veldsdk::log::info!(target: "handlers", "space revoked");
        state.target = None;
        // Without space fetching stands still: no visible rectangle exists and
        // `want_tiles` returns at its very first line. This must be said — otherwise
        // the one showing the list keeps the last reported count, and stopped work
        // looks running and stuck.
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
                // Tile bind groups are built for the previous device's layout —
                // they are incompatible with the new one. The store is emptied: the
                // tiles lie on disk and will return in milliseconds.
                state.tiles = Store::new_like(&state.tiles);
                state.built = None;
                // The emptied store's generation starts over, that is, it stops
                // growing. The old gate key with the same number inside would declare
                // a recomputation redundant over empty space.
                state.checked = None;
                state.device = Some(device);
            }
            Err(error) => {
                // The old target is dropped for the same reason as on the failure
                // below: the owner already freed its texture while allocating this
                // one, and keeping the view means keeping it alive with no viewers.
                veldsdk::log::error!(target: "handlers", "device resources: {:#}", error);
                state.target = None;
                return;
            }
        }
    }

    // Outlines could arrive before the device, and a rebuilt device takes the
    // buffers with it — in both cases they must be poured again.
    upload_outlines(state);

    match Target::create(surface.id, req.width, req.height) {
        Ok(target) => {
            veldsdk::log::info!(target: "handlers",
                "space: texture {} ({}x{})", surface.id, req.width, req.height);
            state.target = Some(target);
        }
        Err(error) => {
            // The old target is dropped: it refers to a texture the owner already
            // freed while allocating this one. There is nowhere to draw into it —
            // showing stopped — and keeping it means keeping both the texture and
            // the depth buffer under it alive.
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
        // A focus without a point is a focus into nowhere: silently looking at the
        // coordinate origin is worse than not moving at all.
        Some(Command::Focus(focus)) => match focus.at {
            Some(at) => state.camera.focus(at.lat, at.lon, focus.radius_deg),
            None => veldsdk::log::warn!(target: "handlers", "focus without a point"),
        },
        None => {}
    }
    // Zooming changes the overlay tile level — ask for what is missing.
    //
    // A focus needs more than this: it only sets the goal, and the camera arrives
    // within half a second, so what is asked from here describes the place it took
    // off from. The frame tick follows it up (see [`on_ui_event`]).
    want_tiles(state, perf::Pass::Camera);
}

/// What is under the pointer. We always answer: "past the Earth" is as much an
/// answer as a point, and the asker has the right to get it. Without space for
/// rendering the answer is the same: there is no frame — hence no frame point to
/// ask about.
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

/// What to outline on the surface. The set as a whole replaces the previous one.
pub fn on_outlines(state: &mut State, outlines: crate::proto::globe::Outlines) {
    state.outlines = outlines.outlines;
    state.generation += 1;
    upload_outlines(state);
}

// ── Overlays ──────────────────────────────────────────────────

/// What to place on the surface. The set as a whole: what was not sent is gone,
/// and its resources are released here. An overlay with the same key and the same
/// resources stays as it is — its tiles and expectations keep living.
pub fn on_overlay(state: &mut State, msg: crate::proto::globe::Overlays) {
    let keep: HashSet<String> = msg.overlays.iter().map(|o| o.key.clone()).collect();
    // Past refusals ended together with the past set: this one was sent already
    // knowing about them.
    state.refused.clear();

    // Removed ones go away: kill production, forget tiles, release resources.
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

    // Layer order is the message order, so the set is rebuilt along it rather than
    // extended. Otherwise a list reordered at the sender would reorder nothing: an
    // accepted overlay would stay at the place of its first arrival, and "move the
    // snapshot up" would silently do nothing.
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

/// The ceiling on the appetite of a single level. The rule is shared with the
/// canvas and lives with the budget (`Store::cap_tiles`); here is only the answer
/// to "how many pyramids are being drawn now".
///
/// Pyramids, not layers: a layer has up to two — preview and detailed — and both
/// lie in the budget. Described ones are counted, not those chosen by the current
/// frame: the choice depends on the ceiling, and deriving the ceiling from it
/// would close them into a loop. The error is then on the safe side — the ceiling
/// is lower, not higher.
///
/// Two ask for it — the one requesting tiles and the one assembling patches — and
/// they must not disagree: the ordered level must be the same one that is drawn.
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

/// How to show a layer: opacity and hiddenness. Unspecified opacity — an opaque
/// layer: proto3's zero default would mean that a sender who does not care about
/// it dims the snapshot in silence (see types.proto).
fn look(incoming: &crate::proto::globe::Overlay) -> (f32, bool) {
    (incoming.opacity.unwrap_or(1.0).clamp(0.0, 1.0), incoming.hidden)
}

/// Accept one overlay from a message. Raster resources arrive into ownership; the
/// same overlay with the same resources is not an event, but how to show it is
/// taken from the new message in any case.
fn adopt_overlay(state: &mut State, incoming: crate::proto::globe::Overlay) {
    if incoming.key.is_empty() {
        veldsdk::log::warn!(target: "handlers", "overlay without a key — resources are released");
        release_rasters(incoming.rasters);
        return;
    }
    let (opacity, hidden) = look(&incoming);

    // The same resources under the same key — the overlay is already ours, the
    // ownership tail stayed with us from the previous message.
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
            // The same resources, another variable at the lying raster — the
            // overlay is the old one, and the raster is described anew with it:
            // its tiles are under another fingerprint, and the old pass is of no
            // use to it. Spares stay with it.
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
                        // Tiles of the old variable are needed by no one: neither
                        // this raster nor the tiler's choice will return under its
                        // fingerprint, and they would occupy video memory until
                        // evicted.
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
            // The rasters are the same — nothing to touch but the display: the
            // opacity slider and "hide" send the same set, and reopening resources
            // for them would mean paying for slider movement with a decode.
            //
            // The frame from the message is discarded meanwhile, deliberately: the
            // layer could have already bound by the raster itself, and the catalogue
            // outline is younger (see `overlay::Binding`). A catalogue-corrected
            // outline arrives only with a change of resources, that is, a new
            // overlay; for the same rasters it will not change either.
            let overlay = &mut state.overlays[index];
            let was = overlay.hidden;
            overlay.opacity = opacity;
            overlay.hidden = hidden;
            // A hidden one asks for no tiles — and neither does the running pass:
            // production per source is single, and holding it for an invisible
            // layer means denying fetching to those that are visible. Showing again
            // asks anew, and what is already fetched is not lost.
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

    // Binding: an exact UTM frame or the declared quad approximation. The rank
    // travels with the frame — its appearance does not reveal it (see
    // `overlay::Binding`).
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
        // Coordinates arrive into ownership together with the raster, and without
        // it they are of use to no one: they must be released on every exit from
        // this loop, otherwise the file stays open until the end of the module's
        // life.
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
        // The grant to the tiler goes before ownership: on refusal the helper
        // releases the resource itself, and wrapping it into an owner would be a
        // second release.
        if let Err(error) = veldsdk::resource::grant_read_or_free(handle.id, "image-tiler") {
            veldsdk::log::warn!(target: "handlers", "{}: raster grant: {}", label, error);
            release_coordinates(raster.geolocation);
            continue;
        }
        // Coordinates are the second resource of the same raster: for Sentinel-3
        // latitude and longitude lie in a neighboring file, and without them the
        // swath has nowhere to lie. They did not open — the raster keeps what it
        // has: it either carries its own binding, or there will be none, and the
        // layer ends with an ordinary refusal.
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
        // A second raster of the same role is a spare of the first, not a
        // neighbor: it is described only when the first one failed to describe
        // (see [`Raster::spares`]).
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

/// The overlay is not accepted. It did not become ours, so the reason lives
/// separately from the set — until the next set (see `State::refused`).
fn refuse(state: &mut State, key: String, label: &str, why: &str) {
    veldsdk::log::warn!(target: "handlers", "{}: {}", label, why);
    state.refused.push((key, why.to_string()));
}

/// Forget the tiles of a fingerprint unless it lies as a description at any raster
/// of any overlay: the same snapshot in another layer keeps them alive.
fn forget_unless_held(state: &mut State, fingerprint: &str) {
    let held = state.overlays.iter().any(|other| {
        other.rasters.iter().any(|raster| raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == fingerprint))
    });
    if !held {
        state.tiles.forget(fingerprint);
    }
}

/// The end of an overlay: kill production, forget tiles (unless the fingerprint
/// lives in another overlay), release resources by their Drop.
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
        // Tiles are forgotten by a different reckoning than the pass is killed: a
        // hidden layer also holds them — showing back is then instant — while
        // there is no reason to work for a hidden one.
        forget_unless_held(state, &fingerprint);
    }
}

/// A raster leaves — together with the layer or into hiding: its pass is carried
/// off with it.
///
/// It is carried off unconditionally, even when a neighboring layer looks at the
/// same file: a pass reads not "the file", but that very resource which is about
/// to be released together with the raster (see `tiles::Passes`). The neighbor
/// will start its own after this one ends.
fn release_pass(state: &mut State, key: &str, role: Role, fingerprint: &str) {
    if let Some(correlation) = state.passes.abandon(fingerprint, &(key.to_string(), role)) {
        crate::cancel::image_tiler::on_produce(&correlation);
    }
}

/// Release the resources of an overlay that will not take root.
/// Release the coordinates of a raster that did not make it into the overlay. A
/// separate function because there are several exits from the acceptance loop,
/// and a file forgotten at any of them stays open until the module is unloaded.
fn release_coordinates(coordinates: Option<veldsdk::proto::core::ResourceHandle>) {
    if let Some(handle) = coordinates {
        veldsdk::resource::release(handle);
    }
}

fn release_rasters(rasters: Vec<crate::proto::globe::OverlayRaster>) {
    for raster in rasters {
        // Coordinates arrive into ownership just like the raster itself, and must
        // be released with the same gesture: left behind, they would keep a file
        // open that no one else would remember.
        for handle in [raster.resource, raster.geolocation].into_iter().flatten() {
            veldsdk::resource::release(handle);
        }
    }
}

// ── Tile pipeline replies ────────────────────────────────────

/// The raster is described — or not: the overlay lives with what it has, the
/// preview and the detailed raster have their own fates.
///
/// Every outcome moves the epoch and recomputes what is needed, even a refusal:
/// until the reply, an overlay with a guessed binding is not drawn at all (see
/// `Overlay::binding_pending`), and a silent exit would leave it invisible until
/// the next camera movement.
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

    // Describing is over — time to report what only now becomes visible.
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == key)
        && !overlay.rasters.iter().any(|raster| raster.describe.is_pending())
    {
        // The binding did remain a guess: the snapshot is about to lie along the
        // catalogue outline, and a rotated snapshot on the globe is explained by
        // nothing else.
        //
        // To the log only: with the four vertices of the footprint lies every
        // snapshot whose raster never spoke about binding — a quicklook, a picture
        // without geo keys — and that is a routine outcome, not a misfortune. A
        // caption on every such layer would be exactly the noise the complaint was
        // started against. A raster that spoke about binding and failed to give it
        // will speak for itself — through `complain`.
        if matches!(overlay.frame, overlay::Frame::Quad(_)) {
            veldsdk::log::warn!(target: "handlers",
                "{}: no binding taken from the rasters — the snapshot lies along the catalogue outline, the order of its vertices need not match the raster walk",
                overlay.label);
        }
        // But if not a single one managed to describe — then the layer has
        // nothing to lie on at all, and there is nothing left to wait for: silence
        // would leave the sender with an eternal "preparing…".
        if overlay.doomed() {
            veldsdk::log::warn!(target: "handlers",
                "{}: no raster described — nothing to overlay", overlay.label);
            // What goes outward is not "failed to describe", but what exactly it
            // is: the reason is known to the tiler and already said in its words
            // ("these are measurements, not an image", "download it and it will
            // show up"), and it is shown by the sender — he has the list and the
            // place for a caption. A general phrase in its place would explain
            // exactly nothing and would take away from the person the only thing
            // he can do about it.
            //
            // There can be two rasters, and their reasons differ: both are named —
            // `trouble` collects them. Empty — there is nothing to say, and then a
            // general phrase is more honest than an empty string.
            let said = overlay.trouble();
            overlay.error = match said.is_empty() {
                true => "no raster described".to_string(),
                false => said,
            };
        } else if !overlay.frame.measurable() {
            // The frame is there, but it has no extent: the catalogue outline
            // collapsed into a point or a line. Neither can cells be drawn by it,
            // nor a level chosen — and left in silence, such a layer hangs
            // "preparing…" and shows nothing.
            veldsdk::log::warn!(target: "handlers",
                "{}: the catalogue outline has no extent — there is nothing to lay the snapshot on",
                overlay.label);
            overlay.error = said_with(
                &overlay.binding_trouble,
                "the snapshot outline collapsed to a point: there is nothing to lay the raster on",
            );
        } else if matches!(overlay.frame, overlay::Frame::Rough(_)) {
            // The space was held in expectation of a grid from the raster, and
            // there was none. The bounding box of a complex outline is not a
            // binding (see `Frame::Rough`), and stretching the snapshot by it
            // would be a lie.
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

/// The layer's refusal together with the reason, if the raster named one.
///
/// Inside the general phrase, not beside it: by `error` the sender removes the
/// layer, and its `trouble`, where the reason lay, goes away with the layer. There
/// will be no other chance to say it — and without it the viewer reads about his
/// own snapshot exactly the same as about any other that failed to lie.
fn said_with(trouble: &Option<String>, general: &str) -> String {
    match trouble {
        Some(said) => format!("{} ({})", general, said),
        None => general.to_string(),
    }
}

/// Recompute the cell balls of every raster of the layer.
///
/// Of every one, not just the described one: the balls are built by the binding,
/// and the binding belongs to the layer and is changed by the description of any
/// of the rasters — the tie-point grid is older than the projection and displaces
/// it (see `Binding`). The preview's leftover balls would then describe cancelled
/// geometry, and cells would be lost silently at that very base which is seen
/// first.
///
/// One place for all the branches of `describe_settled` — it ends entirely with
/// plain `return`s, and every one of them leads here.
fn rebuild_bounds(state: &mut State, key: &str) {
    let Some(at) = state.overlays.iter().position(|o| o.key == key) else { return };
    // Everything is computed before the single mutation: the balls depend on both
    // the layer and the raster, and both are requested by reference, and these two
    // borrows cannot be split any other way.
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

    // The admissibility of a description is decided by a common rule: one tiler,
    // one pyramid, and there is nothing to disagree with the canvas about in which
    // reply counts as admissible. What is ours here alone is what to do with an
    // inadmissible one: an overlay has two rasters, and the one that refused
    // leaves the layer living on the second, so this does not become a layer
    // error. But neither does it become silence: the zoomed-in viewer waits for
    // sharpness that will now never come, and by an empty caption "still coming"
    // cannot be told from "never coming". The reason goes out as a caption beside
    // the progress bar.
    let meta = match tiles::describe(&msg) {
        Ok(meta) => meta,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "{}: raster description: {}", label, error);
            // What to do with a refusal is decided by the raster
            // ([`Raster::refused`]): behind a file that failed to describe there
            // may be a spare — the next one takes its place and describes itself;
            // a refusal of a named variable leaves the file in place with a
            // complaint in the tiler's words.
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

    // Grid nodes are in fractions of the raster: an overlay has two rasters of
    // different sizes, yet both lie the same way, and the binding is one per
    // overlay.
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
    // It described — its own complaint is gone. It does not touch another's: the
    // neighbor's reason did not disappear because of its success.
    raster.trouble = None;

    // Both described — and the detailed turned out no more detailed than the base.
    // The layer does not need it, but this must be said: the zoomed-in viewer
    // waits for sharpness that will not come, and by silence "still coming" cannot
    // be told from "never coming".
    //
    // Released right here as well: cells held back until the end of the pass are
    // released by the raster choice, and the choice will no longer come to this
    // raster — and the layer would stand at "pyramid is being assembled" until it
    // is removed.
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
        // The overlay is re-taken: the borrow above ended together with releasing
        // the pass, and below the layer is needed whole.
        let Some(again) = state.overlays.iter_mut().find(|o| o.key == key) else { return };
        overlay = again;
    }

    // The binding from the raster itself outranks whatever the catalogue said
    // about the snapshot: there it says where it is, here — which pixel goes
    // where. Which one overrides which is decided by the binding kind, not the
    // order of describing (see `overlay::Binding`).
    if !ties.is_empty() {
        match overlay::Grid::new(&ties) {
            Some(grid) => {
                // A four-node grid describes the same linear mapping as the file's
                // projection and does not become older than it (see
                // `overlay::Binding::Lattice`).
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
            // There are points, but no grid came out — this cannot be kept
            // silent: the snapshot will lie along the catalogue outline, that is,
            // most likely rotated.
            //
            // And leaving here is not an option either: the file carries both the
            // tie points and the projection, and points that failed to assemble do
            // not cancel the second. Further down the text it is tried.
            None => {
                let said = format!("{} tie points did not assemble into a grid", ties.len());
                veldsdk::log::warn!(target: "handlers",
                    "{}: {} — the binding stays along the outline", label, said);
                overlay.complain(said);
            }
        }
    }

    // The raster lies in a projection. The system code is interpreted here, not in
    // the tiler: the Krüger series are single in the tree (`projection.rs`), and a
    // second copy of them would agree with the first by eye and diverge on numbers.
    let Some(found) = msg.placement else {
        // The raster brought no binding at all — and if it said why, then that is
        // the answer for the viewer: the snapshot will lie along the catalogue
        // outline, and his question is one — did we fail ourselves or does the
        // file say nothing about the place.
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
    // Field by field, not by position: the names here and there are the same, and
    // a swapped pair is visible right in the line (see `overlay::Placement`).
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
        // The system is named, but there is nothing to translate it with. This
        // must be said in code: by "no binding" our inability cannot be told from
        // the file's silence, and there would be nothing to go on when
        // investigating such a complaint.
        //
        // And it must be said not only to the log: the snapshot will lie along the
        // catalogue outline, that is, most likely rotated, and the one looking at
        // the globe knows nothing of the log.
        Err(why) => {
            veldsdk::log::warn!(target: "handlers",
                "{}: the raster is bound to {} — the binding stays along the outline", label, why);
            overlay.complain(format!("binding not taken: {}", why));
        }
    }
}

/// A tile from the disk cache.
pub fn on_tile(state: &mut State, msg: CachedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// A tile from the producer.
pub fn on_produced(state: &mut State, msg: ProducedTile) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.peek(&correlation) else {
        return tiles::release(msg.texture);
    };
    let (key, role, fingerprint) = (ctx.key.clone(), ctx.role, ctx.fingerprint.clone());
    accept_tile(state, &key, role, &fingerprint, (msg.level, msg.x, msg.y), msg.texture, msg.width, msg.height);
}

/// The cache answered with everything it could; misses go to the producer.
pub fn on_query_done(state: &mut State, msg: QueryDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_query.take(&correlation) else { return };
    let level = ctx.cells.first().map_or(0, |(level, ..)| *level);

    // What the layer needs right now — that is what filters out the misses. While
    // the cache was searching, the camera could move away, and production is
    // single: giving it a cell that has already left the frame occupies it for a
    // minute with what is not visible — while the visible waits all that time.
    // What moved away is not lost: it will be re-asked when it returns into the
    // frame (the canvas counts the same way).
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
        // The layer was hidden while the cache was searching: starting a pass for
        // it would occupy the single production with what is not visible.
        let hidden = overlay.hidden;
        let Some(raster) = overlay.raster_mut(ctx.role) else { return };
        if hidden {
            raster.fetch.forget_asked(&ctx.cells);
            return;
        }

        // The overlay was replaced while the reply was on the way: that request's
        // expectations are not about this raster.
        if raster.meta.as_ref().is_none_or(|meta| meta.fingerprint != ctx.fingerprint) {
            return;
        }
        // A reply settled by the host is the same refusal, and its path is the
        // same: take its cells off the expectations, say it out loud, move the
        // epoch. Taken for a success, it would leave them hanging forever (see
        // `veldsdk::reply::undelivered`).
        let error = veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone());
        if !error.is_empty() {
            raster.fetch.forget_asked(&ctx.cells);
            veldsdk::log::warn!(target: "handlers", "{}: tile cache: {}", label, error);
            // And on screen: the snapshot is alive but will not get sharper, and
            // by silence "still coming" cannot be told from "never coming". The
            // same field and the same words as the canvas has — their work is one.
            raster.trouble = Some(format!("incomplete: {}", error));
            // The epoch is moved here too: without it `build_patches` would exit
            // on the comparison, the fetching progress would no longer be
            // recomputed, and the last reported "work in progress" would stay in
            // force until the end of the run. There is nothing to re-ask meanwhile
            // — a cache refusal is not about the network, but about the request
            // itself, and a retry would give the same answer in a circle.
            state.epoch += 1;
            return;
        }

        // Whether the requested level is read pointwise: on this depends whether
        // to wait silently for the end of one's own pass or to re-ask the cache
        // (a pass puts into it more than it gives the client).
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
            // We wait for someone else's pass in silence: its end will recompute
            // what everyone needs.
            Missed::Waiting => return,
            Missed::Closed => None,
        }
    }) else {
        // Nowhere to go and nothing to wait for: the cache closed the order whole
        // or the rest moved out of sight. What is needed for the current frame
        // must be asked right here — otherwise fetching stops until the nearest
        // camera movement (see `Missed::Closed`).
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

/// The progress of the producer's pass. The only thing that can be said about
/// the work while a sequential source is read through: not a single cell is added
/// in the meantime, and reading can take a minute.
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
    // The epoch is moved: without it the frame tick would exit on the comparison,
    // and the fetching progress would not be recomputed — while it has just
    // changed.
    state.epoch += 1;
}

/// The single end of production, whatever it is — for a killed one the host
/// answers with an empty message.
pub fn on_produce_done(state: &mut State, msg: ProduceDone) {
    let correlation = veldsdk::correlation();
    let Some(ctx) = state.pending_produce.take(&correlation) else { return };
    // The guard is taken down first of all and unconditionally: it stood on the
    // source, not on the raster, and the overlay could be removed while the pass
    // was running. Were we to leave here without taking it down, neighboring
    // rasters of the same file would wait for an end that will never come.
    // If we dropped the pass ourselves, that is a routine move of zooming in, not
    // a failure: the host settles the reply for the dropped one with the same
    // empty message as for the failed one, and only the one who dropped it can
    // tell them apart.
    let ours_kill = state.passes.finish(&correlation);

    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.key == ctx.key) {
        let label = overlay.label.clone();
        if let Some(raster) = overlay.raster_mut(ctx.role) {
            // The overlay was replaced while the pass was running: its cells are
            // already not about this raster, and neither expectations nor a
            // refusal may be attributed to it.
            let ours =
                raster.meta.as_ref().is_some_and(|meta| meta.fingerprint == ctx.fingerprint);
            // An empty reply settled by the host for the failed one — that is a
            // broken-off pass, not a successful one: taken for a success, it would
            // also forgive cells that no one produced (`Fetch::forgive`).
            let error = match ours_kill {
                true => msg.error.clone(),
                false => veldsdk::reply::undelivered(&msg.error).unwrap_or_else(|| msg.error.clone()),
            };
            let failed = ours && !error.is_empty();
            raster.pass = (0, 0);
            raster.fetch.produced(if ours { &ctx.cells } else { &[] }, failed);
            if failed {
                // Do not re-ask what has already failed to be produced: every
                // frame would hammer the producer with the same refusal.
                veldsdk::log::warn!(target: "handlers", "{}: production: {}", label, error);
                raster.trouble = Some(format!("incomplete: {}", error));
            }
        }
    }

    // While the pass was running, requests for this source were held back — and
    // not only at the client: a neighboring layer with the same file waited in
    // silence all that time. The recomputation goes over all overlays, and there
    // is no one else to wake them.
    //
    // The epoch is moved here too: the end of a pass is visible only by it — the
    // pass may bring no tiles at all, while the fetching progress must report it.
    state.epoch += 1;
    want_tiles(state, perf::Pass::Fetch);
}

/// Recompute what every overlay needs: raster and level choice for the look,
/// requesting what is missing from the cache. Production of a single raster is
/// single: while it runs, new requests for it are held back; the goal got coarser
/// below its level — it is killed (rolling a step back is not a goal, see
/// `tiles::Passes::stale`).
///
/// The cause arrives as an argument: there are many call sites, they cost the
/// same, and mean different things (see `perf::Pass`). Counting them as one
/// number would mean being unable to say what caused a spike.
fn want_tiles(state: &mut State, from: perf::Pass) {
    let Some(look) = looking(state) else { return };
    let cap = cap_tiles(state);

    // Hidden ones ask for nothing: a tile that is not visible would evict from the
    // budget one that is visible.
    let keys: Vec<String> = state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .map(|overlay| overlay.key.clone())
        .collect();
    // The measurement accumulates over all overlays and is recorded in one pass:
    // patch assembly counts them all at once, and recorded here one by one they
    // would give a pass per layer against its one — on that very same meter.
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

/// The look by which the desired is measured. `None` — there is no space for the
/// frame yet, and no visible rectangle exists.
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
    // There is no pass — cells held back for its time have nothing left to wait
    // for.
    if !going {
        raster.fetch.resume();
    }
    let Some(cells) = raster.fetch.ask(&state.tiles, &fingerprint, wanted.want.cells) else {
        return;
    };

    // The globe has no status line of its own, and the fetching progress is
    // visible only here: which level of which raster was needed and how many of
    // its cells are missing.
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

/// A tile has arrived — into the store and out of the expectations.
///
/// The cell is taken off the expectations in any case: an answer about it came,
/// and a tile that did not land is a miss, and it will be re-asked by the next
/// recomputation. Were it left in the expectations, it would never be re-asked.
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
        // There is no device — nothing to draw with and nowhere to put it; the
        // cell is not at fault meanwhile and will be re-asked once space for the
        // frame appears.
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
            // The stumble is ours: the cell is not at fault and will be re-asked.
            // The fetching progress does not change because of it — the
            // expectation is simply taken down.
            tiles::Landing::Retry => {
                // A second stumble in a row is already a verdict, and the fetching
                // progress changes exactly as in the branch below.
                if raster.fetch.stumbled(addr) {
                    state.epoch += 1;
                }
            }
            tiles::Landing::Verdict => {
                // A hopeless cell stops holding the step (`Fetch::hopeless`) and
                // is no longer asked, that is, the fetching progress changes. The
                // store generation will not report this: the rejected tile did not
                // land in it and did not move the counter.
                raster.fetch.rejected(addr);
                state.epoch += 1;
            }
        }
    }
}

/// Rebuild of the warp patches when what they are made of has changed: the set of
/// overlays, their choices (raster and level), or the tile store generation.
///
/// The look enters here, but not the geometry itself: patch vertices lie by the
/// binding and do not depend on where one looks from — the look moves only the
/// uniform. But it filters the cells and takes the level, and that is the choice.
fn build_patches(state: &mut State) {
    let Some(look) = looking(state) else {
        // There is nowhere to ripple: without space for the frame nothing is
        // drawn, and a wave left running would call redraw in vain.
        state.ripple = None;
        // No space for the frame — there is nothing to count by it, but there is
        // something to report: a refused layer and a layer that ended in an error
        // are known without a frame. Left in silence, they hang at the sender with
        // "preparing…" until the globe tab is opened — and `on_overlay_progress`
        // is his only path by which a layer is taken down on `error`.
        report_progress(state, &[]);
        return;
    };

    // While the look, the store, and the set of overlays are the same, the choices
    // are the same too — an idle tick does not even build the list for comparison.
    let now = (look, state.tiles.generation, state.epoch);
    if state.checked == Some(now) {
        return;
    }
    state.checked = Some(now);

    let cap = cap_tiles(state);

    // Opacity rides in the list together with the choice, because it is baked into
    // the patches (into a vertex): moving the slider changes no choices, and
    // without it the comparison would decide there is nothing to rebuild.
    //
    // The order of the list is the drawing order, and that is the order of the
    // set: hidden ones drop out of it entirely.
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
    // The measurement is recorded here, not at the end of the function: below
    // there is an exit on a matching `built`, and a pass recorded after it would
    // be lost in exactly the most common case — when the walk is counted and the
    // choice has not changed.
    state.perf.pass(perf::Pass::Patches, &toll, began.elapsed(), Instant::now());
    // The fetching progress is counted here as well, from the same list: it is
    // asked about the same thing — what the overlay needs right now — and counted
    // separately it would diverge from what is drawn before the eyes of the one
    // looking at the list.
    report_progress(state, &wanted);

    // The carriers are asked before the fingerprint comparison, not in the
    // assembly itself: the fingerprint is measured by them. While the cells are
    // covered by the same tiles, the vertices would come out the same, however
    // much the store changes around (see `Built::cells`).
    //
    // It costs a walk of the ancestor chain per cell, and it is now paid on every
    // tick that passed the gates, not only on a rebuild. The lookups meanwhile
    // extend the tiles' life in the budget — but not for that reason: eviction
    // happens only on arrival (`Store::accept`), and an arrival moves the
    // generation and opens the gates by itself.
    let mut pieces: Vec<Vec<overlay::Piece>> = Vec::with_capacity(wanted.len());
    {
        let State { overlays, tiles, .. } = &mut *state;
        for (key, _, wanted) in &wanted {
            let found = overlays.iter().find(|o| &o.key == key);
            pieces.push(found.map_or_else(Vec::new, |o| overlay::pieces(o, wanted, tiles)));
        }
    }

    // Ripple strength — before the fingerprint comparison: the cells are already
    // counted, and below lies an exit on a match, past which there would be
    // nowhere to take it from. The largest across the layers: there is one ripple
    // per frame, and the layer that has to travel the longest sets it entirely.
    state.ripple = wanted
        .iter()
        .zip(&pieces)
        .filter(|(_, pieces)| pieces.iter().any(overlay::Piece::coming))
        .filter_map(|((key, ..), _)| state.overlays.iter().find(|o| &o.key == key))
        .map(|overlay| ripple_strength(overlay.progress.within))
        .max_by(f32::total_cmp);

    // What went into the fingerprint and why — up to the fingerprint itself
    // (`Overlay::built`).
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
    // The measurement is here, not right after the assembly, and before the
    // outcome is inspected: the fill is part of the same rebuild, and as much goes
    // into it as was assembled, while a failed one cost exactly as much as a
    // successful one.
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

/// The period of the ripple wave in seconds: this is how long one run through the
/// phase takes.
const RIPPLE_PERIOD_S: f32 = 1.6;

/// Into how many parts of the period the phase is snapped in the frame
/// fingerprint. Thirty steps a second is the limit past which the eye stops
/// distinguishing wave movement, and frames beyond it would be redraws for one and
/// the same thing.
const RIPPLE_STEPS: f32 = RIPPLE_PERIOD_S * 30.0;

/// Where the phase went during this tick and how the frame differs from the
/// previous one.
///
/// A pure function, because it guards against the main danger of the edit — an
/// eternally drawing globe: while there is nothing to ripple, the snapped phase
/// must stand still, otherwise frame comparison will stop skipping the stillness.
fn advance_ripple(phase: f32, dt: f32, ripple: Option<f32>) -> (f32, u32) {
    let phase = match ripple {
        Some(_) => (phase + dt / RIPPLE_PERIOD_S).fract(),
        None => phase,
    };
    (phase, (phase * RIPPLE_STEPS) as u32)
}

/// How noticeable the ripple is on a step that has reached such a fraction.
///
/// Not by the fraction itself: a step just started runs no less than one about to
/// end, and an invisible ripple at its beginning would mean "nothing is happening"
/// exactly where the wait is longest. Therefore the fraction does not set the
/// strength, but adds to its base.
fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Recompute the fetching progress from what the layer needs right now, and send
/// the set as a whole.
///
/// It is measured by two quantities, and both are derived from the order, not
/// from the frame. The share of the way is the pyramid steps: from the apex they
/// start at to the level at which the picture becomes sharp; each is closed once,
/// and panning does not move them. The cell count is the last placed order
/// (`tiles::Fetch::ordered`), not what got into the frame on this frame. Counted
/// by what is visible it rides back and forth on every camera movement and reports
/// nothing.
///
/// Empty `wanted` — there is nothing to count (no space for the frame, the layer
/// is hidden, or the raster is still being described): then the layer is reported
/// as the last live frame counted it. Work is meanwhile not declared stopped:
/// describing runs without a frame too, and it is also the longest part of the
/// raster's way over the network (see `Overlay::working`).
///
/// Only the changed set goes out — the topic is declared a snapshot, and a repeat
/// cuts off its stub (see schema.yaml).
fn report_progress(state: &mut State, wanted: &[(String, f32, overlay::Wanted)]) {
    for overlay in &mut state.overlays {
        // The last entry, not the sum: preview and detailed are two different
        // pyramids with different fingerprints, and summed into one count they
        // change the denominator at the very instant the screen pixel crosses the
        // preview's native resolution. The last one brings the sharpness, and it is
        // the one being talked about.
        let Some((.., mine)) = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back()
        else {
            continue;
        };
        let Some(raster) = overlay.rasters.iter().find(|raster| raster.role == mine.choice.role)
        else {
            continue;
        };
        // Bytes are taken from the same raster as the steps: a layer may have two
        // rasters, and numbers summed from different pyramids would report two
        // different jobs at once.
        overlay.progress = overlay::progress_of(raster, &mine.want);
    }

    let overlays: Vec<OverlayProgress> = state
        .overlays
        .iter()
        .map(|overlay| {
            // A layer that is not allowed to count (hidden, no space for the
            // frame) has nothing to fetch — hence `live`. Describing does not go
            // into this count: it does not ask the frame (see `Overlay::working`).
            // Everything else is decided by the common rule (`tiles::working`): it
            // is exactly "the way is not walked", not "something is in flight".
            //
            // A second sign beside it is absent deliberately, though it suggests
            // itself: `share < 1.0` — the same fact written as a fraction. The
            // share within a step reaches one exactly when the expectations empty
            // (`Fetch::ordered` counts as arrived even what was refused), so the
            // fraction is below one exactly when "cells are in flight or the step
            // is not the last". Keeping this in two ways would mean one day fixing
            // one and not the other.
            let mine = wanted.iter().filter(|(key, ..)| key == &overlay.key).next_back();
            overlay.report(mine.map(|(.., wanted)| wanted), &state.passes)
        })
        // Refused ones — in the same lines: we have no such overlay, but it must
        // be reported, otherwise the sender will consider it lying on the globe
        // forever.
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

/// Rebuilds the outline geometry and fills it into the device buffers.
///
/// Without a device there is nothing to do and nothing to fear: the set lies in
/// the state, and the delegation of space that creates the device will fill it.
fn upload_outlines(state: &mut State) {
    let State { device: Some(device), outlines, .. } = state else { return };
    let built = outlines::Outlines::build(outlines);
    // The numbers are useful exactly for investigating "why are they not seen":
    // outlines can be small (a Sentinel-2 cell is about a degree) and can be over
    // the horizon, and there is nothing else to tell that from "they have not
    // arrived".
    veldsdk::log::debug!(target: "render",
        "outlines: {} of them, {} line vertices, {} ribbon vertices, {} hatch vertices",
        outlines.len(), built.vertices.len(), built.ribbon.len(), built.hatch.len());
    if let Err(error) = device.set_outlines(&built) {
        veldsdk::log::error!(target: "render", "outlines not filled: {:#}", error);
    }
}

/// The frame tick. Other window events are not ours: the cursor and keys are
/// handled by the one who draws the layout, and what reaches us is already
/// handled.
pub fn on_ui_event(state: &mut State, event: app_proto::UiEvent) {
    let Some(app_proto::ui_event::Event::Frame(frame)) = event.event else {
        return;
    };
    // The meter's report — here, before everything else: below, the function ends
    // with three exits (no device, the frame matched the drawn one, write
    // failure), and a counter placed past any of them would count not frames, but
    // a subset of frames. Placing it higher is not an option either: cursor
    // movements and scrolling go into this topic too, and they would inflate the
    // count exactly during a gesture, that is, right where one looks at it.
    if let Some(report) = state.perf.frame(Instant::now()) {
        veldsdk::log::debug!(target: "perf", "{}", report);
    }
    // The aim at the snapshot rides, not jumps, and the tick leads it: it only
    // sets the goal (see `Camera::focus`). A moved camera is seen below by
    // comparison with the drawn frame — no separate "redraw" is needed — but the
    // overlay tiles must be asked for: the level and the set of visible cells are
    // computed from the camera, and what was ordered before the flight describes
    // the place it took off from. The same gesture as with a gesture — there
    // `want_tiles` is called on every event, and here on every frame of the flight.
    if state.camera.advance(frame.dt) {
        want_tiles(state, perf::Pass::Camera);
    }
    // Overlay patches — before the frame comparison: arrived tiles and a level
    // change are visible as their rebuild, and it also moves the counter in
    // [`Frame`]. It also answers whether there is anything to ripple in the frame.
    build_patches(state);

    // The phase moves only when there is something to ripple, and the snapped one
    // goes into the frame fingerprint — that is what turns redraw on exactly for
    // the loading time.
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
