//! One canvas: what it shows, where the camera looks, and how a frame is
//! assembled from tiles.
//!
//! A frame cell is the tile of the current level. It is drawn by exactly one
//! quad: the exact tile, or while that one is missing — a piece of the nearest
//! available ancestor (pyramid addressing allows this: a level z+1 tile covers
//! four level z tiles, see resample::halve at the tiler). One cell — one quad,
//! so there are no overlaps and no depth buffer is needed.

use veldmap_image_tiler_wrap::pyramid::{self, TILE};
use veldmap_image_tiler_wrap::tiles::{self, Fetch, Meta, Passes, Store};
use veldsdk::graphics::GrowingBuffer;

use super::camera::Camera;
use super::gpu::{self, Quad, Target};

pub struct View {
    pub label: String,
    /// Named variable of the file (`ShowRequest.variable`); empty means the
    /// tiler's choice. It travels into the describe and into every pass: the
    /// tiler's parsing is keyed by it.
    pub variable: String,
    /// Plane of the previous variable while another one is being described: the
    /// camera survives from the previous one and stays valid only if the plane
    /// is the same — otherwise fit anew (see [`View::plane_changed`]).
    pub previous_plane: Option<(u32, u32)>,
    pub target: Option<Target>,
    pub shown: Option<Shown>,
    /// The camera appears at the first fit — once both the image and the canvas
    /// size are known — and afterwards lives its own life.
    pub camera: Option<Camera>,
    /// Two vertex buffers alternating per frame (see [`View::vertices`]).
    ///
    /// Vertices go into the buffer immediately, by a host command, while the
    /// host executes the draw commands later, in its frame loop — so a second
    /// frame written before the first one executes would overwrite the vertices
    /// the first still references: old commands with their tile bind groups
    /// would draw the new frame's quads, and tiles would briefly land in the
    /// wrong places. A frame written to the other buffer leaves the previous
    /// one alone; `turn` alternates them. Two suffice for one lagging frame: a
    /// guest that processes two or more `Frame` messages between the write and
    /// the execution writes again into the buffer of the unexecuted one — the
    /// actor queue is unbounded, and a ring sized to its depth would cost a
    /// buffer per frame.
    pub vertices: [GrowingBuffer; 2],
    /// Whose buffer the next frame uses.
    pub turn: usize,
    /// What the last written frame was made of; if it matches, the frame is skipped.
    pub drawn: Option<Stamp>,
    /// What is already asked for, what is being produced, and what must not be
    /// asked again — the shared tile consumer bookkeeping (see
    /// `veldmap_image_tiler_wrap::tiles`).
    pub fetch: Fetch,
    /// Freshness of the description: the shown resource may change while the
    /// reply is in flight.
    pub describe: veldsdk::Latest,
    pub read_bytes: u64,
    pub total_bytes: u64,
    /// Nothing to look at: the resource failed to open or to describe.
    pub error: Option<String>,
    /// The frame is incomplete: a pass broke off, the cache refused. The image
    /// itself is alive, and showing the reason instead of it would be untrue —
    /// while keeping it forever is pointless: the first landed tile clears it
    /// (see [`View::landed`]).
    ///
    /// The word about the reason is spoken here in full: both complaints travel
    /// over the wire in one field, and the client shows what is said as-is.
    pub trouble: Option<String>,
    /// The last quad assembly had something to ripple. A field, not an answer
    /// computed on the spot: the frame stamp is compared before the quads are
    /// computed, while the decision to move the phase must be made before that
    /// comparison. Being one frame late is harmless here: while the ripple runs,
    /// the quads are recomputed every tick and the flag updates along with them,
    /// and once it goes dark it stops the recomputation too.
    pub glowing: bool,
    /// The frame failed to record at all — a complaint about the render itself,
    /// not the pipeline. Separate from `trouble` because different events clear
    /// them: a landed tile clears the pipeline one, a successful frame clears
    /// this one. In one field they would cancel each other: a successful
    /// repaint of a hole would erase "did not arrive", and a landed tile would
    /// erase "the frame is stuck", which has not gone anywhere.
    pub stuck: Option<String>,
}

/// The shown resource. Metadata arrives in a second beat (on_described), and
/// until then the image "is there, but there is nothing to draw".
pub struct Shown {
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
}


/// What a frame is assembled from. The tile store generation replaces comparing
/// the sets themselves: any arrived or evicted tile moves it.
#[derive(Clone, Copy, PartialEq)]
pub struct Stamp {
    pub camera: Camera,
    pub generation: u64,
    pub texture: u64,
    /// Ripple phase quantized to a display step. While there is nothing to
    /// ripple it stands still — and the frame is skipped exactly as it would be
    /// without it.
    pub ripple: u32,
}

impl View {
    pub fn new(label: String) -> Self {
        Self {
            label,
            variable: String::new(),
            previous_plane: None,
            target: None,
            shown: None,
            camera: None,
            vertices: [gpu::vertex_buffer(), gpu::vertex_buffer()],
            turn: 0,
            drawn: None,
            fetch: Fetch::default(),
            describe: veldsdk::Latest::default(),
            read_bytes: 0,
            total_bytes: 0,
            error: None,
            trouble: None,
            stuck: None,
            glowing: false,
        }
    }

    pub fn meta(&self) -> Option<&Meta> {
        self.shown.as_ref()?.meta.as_ref()
    }

    /// Whether the plane changed along with the description of another
    /// variable: a file's candidates are any two-dimensional variables, and
    /// next to a 1500-column strip sits a variable of a different grid. A
    /// camera fitted for the previous one would show the new one off-target;
    /// asking consumes the previous plane — it is forgotten in the process.
    pub fn plane_changed(&mut self, meta: &Meta) -> bool {
        self.previous_plane.take().is_some_and(|(width, height)| (width, height) != (meta.width, meta.height))
    }

    /// A tile landed — the complaint about the past is cleared. The picture is
    /// being completed right now, and a "did not arrive" caption next to it
    /// would speak of something that is already gone.
    pub fn landed(&mut self) {
        self.trouble = None;
    }

    /// The complaint on the wire: one line, three causes, ordered by urgency. A
    /// stuck frame outranks incompleteness — an incomplete frame at least draws;
    /// incompleteness outranks the detail cap — it is about now, while the cap
    /// is not going anywhere.
    ///
    /// The detail cap (`Meta::capped`) is computed here rather than stored in
    /// [`View::trouble`] because different events clear them: `trouble` is
    /// cleared by the first landed tile ([`View::landed`]), while the detail cap
    /// is a property of the source itself and only a new description can clear
    /// it. Were it stored in the same field, it would flash and vanish with the
    /// first tile — exactly when the viewer starts examining the picture.
    ///
    /// `settled` — the canvas is showing and there is no work behind it;
    /// otherwise the cap stays quiet. The client shows the complaint **instead
    /// of** the fetch progress (`preview.rs`), and for JP2 both rungs of the
    /// ladder mean a full file read — tens of seconds over an empty canvas.
    /// "No more detail" over an empty canvas reads as "loading is done, here is
    /// its ceiling", answering a question that was not asked.
    ///
    /// Mere idleness is not enough, and this is measured: a described image that
    /// has been given neither a place nor a camera wants nothing at all — and
    /// with nothing to want there is no work either. Hence both halves: the
    /// showing is going (`want` exists) and everything for it is fetched. The
    /// overlay on the globe computes `settled` the same way for the same reason
    /// (`Overlay::said`).
    pub fn said(&self, settled: bool) -> String {
        self.stuck
            .clone()
            .or_else(|| self.trouble.clone())
            .or_else(|| match settled {
                true => self.meta()?.capped(),
                false => None,
            })
            .unwrap_or_default()
    }

    /// The showing is going: the description is in flight or there is still work
    /// on the pyramid. The latter is computed by the shared rule
    /// (`tiles::working`) — the same one the overlay on the globe answers this
    /// question by.
    pub fn working<K: PartialEq>(&self, passes: &Passes<K>, want: Option<&tiles::Want>) -> bool {
        self.describe.is_pending()
            || self.meta().is_some_and(|meta| {
                tiles::working(&self.fetch, passes, &meta.fingerprint, want)
            })
    }
}

/// What the canvas needs right now: the target, the rung toward it, and its
/// cells. `None` — there is nothing to draw yet (no place, image, or camera).
///
/// The shared rule computes this (`tiles::want`) — the same one the overlay on
/// the globe picks its level by. The canvas brings one thing of its own: the
/// level for the camera scale and the cells covering the visible rectangle.
///
/// It answers both — the tile request and the frame assembly: drawing something
/// other than what was asked for means either requesting the invisible or
/// hiding what was fetched.
pub fn wanted(view: &View, store: &Store, cap: u64) -> Option<tiles::Want> {
    let (target, camera, meta) = parts(view)?;
    let rect = camera.visible((meta.width, meta.height), (target.width, target.height));
    let empty = rect.0 >= rect.2 || rect.1 >= rect.3;

    Some(tiles::want(
        camera.level(meta.levels()),
        meta,
        cap,
        store,
        &view.fetch,
        |level| match empty {
            true => Vec::new(),
            false => {
                let (xs, ys) = cell_range(rect, level, meta);
                ys.flat_map(|y| xs.clone().map(move |x| (level, x, y))).collect()
            }
        },
    ))
}

/// Frame quads: one per visible cell that has a carrier — the exact tile or the
/// nearest available ancestor (`Store::carrier`, shared with the overlays).
/// Lookups extend the tiles' life in the store budget.
pub fn quads(view: &View, store: &mut Store, cap: u64, phase: f32) -> Vec<Quad> {
    let Some((target, camera, meta)) = parts(view) else { return Vec::new() };
    let Some(want) = wanted(view, store, cap) else { return Vec::new() };

    // The ripple strength is the share within the current rung, read by the same
    // rule as on the globe: the rung ripples, not an individual cell, and about
    // a specific cell exactly one bit is known — whether it is on its way or not.
    let inside = tiles::inside(
        view.fetch.ordered(),
        (view.read_bytes, view.total_bytes),
        meta.pointwise(want.level),
    );
    let strength = ripple_strength(inside.share as f32);

    let mut quads = Vec::with_capacity(want.cells.len());
    for cell in want.cells {
        let Some((addr, stored)) = store.carrier(&meta.fingerprint, cell, meta.levels()) else {
            continue;
        };
        let (bind, tex) = (stored.bind.clone(), (stored.width, stored.height));
        let rect = pyramid::cell_image_rect(cell.1, cell.2, want.level, meta.width, meta.height);
        quads.push(Quad {
            rect: to_ndc(rect, camera, target),
            uv: pyramid::cell_uv(rect, addr, tex.0, tex.1),
            bind,
            // A cell covered by its own tile does not ripple, even while it is
            // being waited for: its own tile leaves the wait by arriving.
            glow: match addr != cell && view.fetch.coming(cell) {
                true => glow_of(cell, phase, strength),
                false => 0.0,
            },
        });
    }
    quads
}

/// Ripple wave period in seconds and its blend depth — the same as on the globe
/// (`module::RIPPLE_PERIOD_S`, `RIPPLE_DEPTH` in globe.wgsl): the work is the
/// same, and a different speed on two screens would read as different events.
pub const RIPPLE_PERIOD_S: f32 = 1.6;
const RIPPLE_DEPTH: f32 = 0.30;

/// How many distinct phase offsets are handed to cells — as many as on the
/// globe: neighbors diverge visibly while the ripple does not fall apart into
/// noise.
const RIPPLE_OFFSETS: u32 = 16;

/// How visible the ripple is on a rung that has reached this share.
///
/// Not the share itself: a just-started rung is in no less motion than one
/// about to finish, and an invisible ripple at its start would mean "nothing is
/// happening" exactly where the wait is longest.
fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// The ready blend strength for a cell: the wave sampled at its phase offset.
///
/// The offset derives from the address, not the traversal order: cell order
/// changes on every camera move, and the wave would jump around on a still
/// picture.
fn glow_of(cell: tiles::Addr, phase: f32, strength: f32) -> f32 {
    let (_, x, y) = cell;
    let slot = (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(11))) % RIPPLE_OFFSETS;
    let offset = (slot as f32 + 0.5) / RIPPLE_OFFSETS as f32;
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * (phase + offset)).sin();
    RIPPLE_DEPTH * strength * wave
}

fn parts(view: &View) -> Option<(&Target, &Camera, &Meta)> {
    Some((view.target.as_ref()?, view.camera.as_ref()?, view.meta()?))
}

/// Cell ranges of a level covered by the image rectangle. The cell arithmetic
/// itself (rectangle, carrier, UV) lives in the shared pyramid.rs: the globe
/// computes with it too, and their rules cannot diverge.
fn cell_range(
    rect: (f64, f64, f64, f64),
    level: u32,
    meta: &Meta,
) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let cell = f64::from(TILE) * pyramid::level_px(level);
    let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
    let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
    let x0 = ((rect.0 / cell).floor() as u32).min(grid_w);
    let x1 = ((rect.2 / cell).ceil() as u32).min(grid_w);
    let y0 = ((rect.1 / cell).floor() as u32).min(grid_h);
    let y1 = ((rect.3 / cell).ceil() as u32).min(grid_h);
    (x0..x1, y0..y1)
}

/// Image pixels → frame NDC. Y is flipped: the image goes down, NDC goes up.
fn to_ndc(cell: [f64; 4], camera: &Camera, target: &Target) -> [f32; 4] {
    let (w, h) = (f64::from(target.width), f64::from(target.height));
    let sx = |ix: f64| (ix - camera.cx) * camera.scale + w / 2.0;
    let sy = |iy: f64| (iy - camera.cy) * camera.scale + h / 2.0;
    [
        (sx(cell[0]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[1]) / h * 2.0) as f32,
        (sx(cell[2]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[3]) / h * 2.0) as f32,
    ]
}
