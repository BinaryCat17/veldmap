use veldmap_image_tiler_wrap::pyramid::{self, TILE};
use veldmap_image_tiler_wrap::tiles::{self, Fetch, Meta, Passes, Store};
use veldsdk::graphics::GrowingBuffer;

use super::camera::Camera;
use super::gpu::{self, Quad, Target};

pub struct View {
    pub label: String,
    pub variable: String,
    pub previous_plane: Option<(u32, u32)>,
    pub target: Option<Target>,
    pub shown: Option<Shown>,
    pub camera: Option<Camera>,
    pub vertices: [GrowingBuffer; 2],
    pub turn: usize,
    pub drawn: Option<Stamp>,
    pub fetch: Fetch,
    pub describe: veldsdk::Latest,
    pub read_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
    pub trouble: Option<String>,
    pub glowing: bool,
    pub stuck: Option<String>,
}

pub struct Shown {
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Stamp {
    pub camera: Camera,
    pub generation: u64,
    pub texture: u64,
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

    pub fn plane_changed(&mut self, meta: &Meta) -> bool {
        self.previous_plane.take().is_some_and(|(width, height)| (width, height) != (meta.width, meta.height))
    }

    pub fn landed(&mut self) {
        self.trouble = None;
    }

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

    pub fn working<K: PartialEq>(&self, passes: &Passes<K>, want: Option<&tiles::Want>) -> bool {
        self.describe.is_pending()
            || self.meta().is_some_and(|meta| {
                tiles::working(&self.fetch, passes, &meta.fingerprint, want)
            })
    }
}

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

pub fn quads(view: &View, store: &mut Store, cap: u64, phase: f32) -> Vec<Quad> {
    let Some((target, camera, meta)) = parts(view) else { return Vec::new() };
    let Some(want) = wanted(view, store, cap) else { return Vec::new() };

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
            glow: match addr != cell && view.fetch.coming(cell) {
                true => glow_of(cell, phase, strength),
                false => 0.0,
            },
        });
    }
    quads
}

pub const RIPPLE_PERIOD_S: f32 = 1.6;
const RIPPLE_DEPTH: f32 = 0.30;

const RIPPLE_OFFSETS: u32 = 16;

fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

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
