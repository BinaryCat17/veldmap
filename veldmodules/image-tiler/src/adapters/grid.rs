use std::collections::VecDeque;

use super::super::budget::{self, Peak};
use super::super::cascade::{self, Cascade, Emit};
use super::super::pyramid::{self, TILE};
use super::super::resample::{resample_window, Window};

pub(super) const CHUNK_CACHE_BYTES: usize = budget::CHUNK_CACHE as usize;

pub(super) const REGION_CAP: u64 = budget::CHUNK_CACHE / 2 / 4;

pub(super) const MIN_CHUNK_PIXELS: u64 = 32 * 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub image: usize,
    pub region: u64,
    pub held: u64,
}

pub struct Overview {
    pub image: usize,
    pub width: u32,
    pub height: u32,
    pub chunk: (u32, u32),
}

pub struct Grid {
    pub tiled: bool,
    pub chunk: (u32, u32),
    pub overviews: Vec<Overview>,
    pub depth: u32,
}

impl Grid {
    pub fn random_access(&self) -> bool {
        self.tiled && !self.overviews.is_empty()
    }

    pub fn chunk_of(&self, image: usize) -> (u32, u32) {
        match self.overviews.iter().find(|overview| overview.image == image) {
            Some(overview) => overview.chunk,
            None => self.chunk,
        }
    }

    pub fn source_for(&self, base: (u32, u32), level_width: u32, level_height: u32) -> (usize, u32, u32) {
        let fits = |copy: u32, level: u32| {
            copy >= level.saturating_sub(1) && u64::from(copy) * 2 > u64::from(level)
        };

        let mut best = (0usize, base.0, base.1);
        for overview in &self.overviews {
            if fits(overview.width, level_width)
                && fits(overview.height, level_height)
                && overview.width < best.1
            {
                best = (overview.image, overview.width, overview.height);
            }
        }
        best
    }

    pub fn pointwise(&self, base: (u32, u32), level: u32) -> bool {
        self.footprint(base, level).is_some()
    }

    pub fn footprint(&self, base: (u32, u32), level: u32) -> Option<Footprint> {
        let lw = pyramid::level_size(base.0, level).max(1);
        let lh = pyramid::level_size(base.1, level).max(1);
        let (image, sw, sh) = self.source_for(base, lw, lh);
        let (cw, ch) = self.chunk_of(image);
        if u64::from(cw) * u64::from(ch) < MIN_CHUNK_PIXELS {
            return None;
        }
        let aligned = lw.abs_diff(sw) <= 1 && lh.abs_diff(sh) <= 1;

        let side = |level_side: u32, source_side: u32| -> u64 {
            let ideal = (u64::from(TILE) * u64::from(source_side)).div_ceil(u64::from(level_side));
            match aligned {
                true => ideal,
                false => ideal + 1,
            }
        };
        let rw = side(lw, sw).min(u64::from(sw));
        let rh = side(lh, sh).min(u64::from(sh));
        let region = rw.saturating_mul(rh);
        if region > REGION_CAP {
            return None;
        }

        let held = spanned(rw, cw, sw, aligned).saturating_mul(spanned(rh, ch, sh, aligned));
        if held.saturating_mul(4) > CHUNK_CACHE_BYTES as u64 {
            return None;
        }
        Some(Footprint { image, region, held })
    }

    pub fn chunk_bytes(&self, chunk: (u32, u32)) -> u64 {
        u64::from(chunk.0) * u64::from(chunk.1) * (u64::from(self.depth) + 4)
    }

    pub fn direct_peak(&self, base: (u32, u32), level: u32) -> Option<Peak> {
        let footprint = self.footprint(base, level)?;
        let tile = u64::from(TILE) * u64::from(TILE) * 4;
        Some(
            Peak::new()
                .with("chunk cache", budget::CHUNK_CACHE)
                .with("region", footprint.region * 4)
                .with("fresh chunk", self.chunk_bytes(self.chunk_of(footprint.image)))
                .with("tile", tile * 2),
        )
    }

    pub fn pass_peak(&self, base: (u32, u32)) -> Peak {
        let (width, height) = base;
        Peak::new()
            .with("band", u64::from(width) * u64::from(self.chunk.1) * 4)
            .with("fresh chunk", self.chunk_bytes(self.chunk))
            .with("cascade", cascade::bytes(width, height))
    }
}

pub(super) fn spanned(side: u64, chunk: u32, source: u32, aligned: bool) -> u64 {
    if side == 0 {
        return 0;
    }
    let chunk = u64::from(chunk.max(1));
    let tile = u64::from(TILE);
    let flush = aligned && (chunk % tile == 0 || tile % chunk == 0);
    let offset = match flush {
        true => chunk - tile.min(chunk),
        false => chunk - 1,
    };
    let touched = ((offset + side - 1) / chunk + 1).min(u64::from(source).div_ceil(chunk));
    touched.saturating_mul(chunk)
}

pub(super) struct Region {
    pub(super) sx0: u64,
    pub(super) sx1: u64,
    pub(super) sy0: u64,
    pub(super) sy1: u64,
    pub(super) window: Window,
}

pub(super) fn region_of(
    (tx, ty): (u32, u32),
    (tw, th): (u32, u32),
    (lw, lh): (u32, u32),
    (sw, sh): (u32, u32),
    aligned: bool,
) -> Region {
    let (x0, y0) = (u64::from(tx) * u64::from(TILE), u64::from(ty) * u64::from(TILE));
    if aligned {
        let (x0, y0) = (x0.min(u64::from(sw).saturating_sub(1)), y0.min(u64::from(sh).saturating_sub(1)));
        let (x1, y1) =
            ((x0 + u64::from(tw)).min(u64::from(sw)), (y0 + u64::from(th)).min(u64::from(sh)));
        let window = Window { x0: 0.0, y0: 0.0, x1: (x1 - x0) as f64, y1: (y1 - y0) as f64 };
        return Region { sx0: x0, sx1: x1, sy0: y0, sy1: y1, window };
    }

    let exact = |at: u64, side: u32, level_side: u32| {
        at as f64 * f64::from(side) / f64::from(level_side)
    };
    let sx0 = (x0 * u64::from(sw)) / u64::from(lw);
    let sy0 = (y0 * u64::from(sh)) / u64::from(lh);
    let sx1 = (u64::from(tx * TILE + tw) * u64::from(sw)).div_ceil(u64::from(lw)).min(u64::from(sw));
    let sy1 = (u64::from(ty * TILE + th) * u64::from(sh)).div_ceil(u64::from(lh)).min(u64::from(sh));
    let window = Window {
        x0: exact(x0, sw, lw) - sx0 as f64,
        y0: exact(y0, sh, lh) - sy0 as f64,
        x1: exact(u64::from(tx * TILE + tw), sw, lw) - sx0 as f64,
        y1: exact(u64::from(ty * TILE + th), sh, lh) - sy0 as f64,
    };
    Region { sx0, sx1, sy0, sy1, window }
}

pub trait Chunked {
    fn chunk(&mut self, image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String>;

    fn prefetch(&mut self, _image: usize, _indices: &[u32]) -> Result<(), String> {
        Ok(())
    }
}

struct ChunkCache {
    entries: VecDeque<(u32, Vec<u8>, u32, u32)>,
    bytes: usize,
    asked: u32,
    hits: u32,
}

impl ChunkCache {
    fn new() -> Self {
        Self { entries: VecDeque::new(), bytes: 0, asked: 0, hits: 0 }
    }

    fn get<S: Chunked>(
        &mut self,
        source: &mut S,
        image: usize,
        index: u32,
    ) -> Result<(&[u8], u32, u32), String> {
        self.asked += 1;
        if let Some(at) = self.entries.iter().position(|(i, ..)| *i == index) {
            self.hits += 1;
            let (_, data, dw, dh) = &self.entries[at];
            return Ok((data, *dw, *dh));
        }

        let (rgba, dw, dh) = source.chunk(image, index)?;

        self.bytes += rgba.len();
        self.entries.push_back((index, rgba, dw, dh));
        while self.bytes > CHUNK_CACHE_BYTES && self.entries.len() > 1 {
            if let Some((_, old, ..)) = self.entries.pop_front() {
                self.bytes -= old.len();
            }
        }
        let last = self.entries.back().unwrap();
        Ok((&last.1, last.2, last.3))
    }
}

pub fn direct<S: Chunked>(
    source: &mut S,
    grid: &Grid,
    base: (u32, u32),
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let lw = pyramid::level_size(base.0, level);
    let lh = pyramid::level_size(base.1, level);
    let (image, sw, sh) = grid.source_for(base, lw, lh);
    let (cw, ch) = grid.chunk_of(image);
    let across = sw.div_ceil(cw);
    let mut chunks = ChunkCache::new();

    let aligned = lw.abs_diff(sw) <= 1 && lh.abs_diff(sh) <= 1;
    let began = std::time::Instant::now();

    veldsdk::log::debug!(target: "decode",
        "pointwise access: level {} ({}×{}) from image {} ({}×{}), {} tiles, grid {}",
        level, lw, lh, image, sw, sh, wants.len(),
        if aligned { "own" } else { "foreign" });

    let mut needed = std::collections::BTreeSet::new();
    for &(tx, ty) in wants {
        let (tw, th) = (pyramid::tile_extent(tx, lw), pyramid::tile_extent(ty, lh));
        let Region { sx0, sx1, sy0, sy1, .. } = region_of((tx, ty), (tw, th), (lw, lh), (sw, sh), aligned);
        if sx1 <= sx0 || sy1 <= sy0 {
            continue;
        }
        for cy in sy0 / u64::from(ch)..=(sy1 - 1) / u64::from(ch) {
            for cx in sx0 / u64::from(cw)..=(sx1 - 1) / u64::from(cw) {
                needed.insert((cy * u64::from(across) + cx) as u32);
            }
        }
    }
    let needed: Vec<u32> = needed.into_iter().collect();
    if let Err(why) = source.prefetch(image, &needed) {
        veldsdk::log::debug!(target: "decode", "chunks not prefetched, reading on misses: {}", why);
    }

    for &(tx, ty) in wants {
        let tw = pyramid::tile_extent(tx, lw);
        let th = pyramid::tile_extent(ty, lh);

        let Region { sx0, sx1, sy0, sy1, window } =
            region_of((tx, ty), (tw, th), (lw, lh), (sw, sh), aligned);
        let (rw, rh) = ((sx1 - sx0) as u32, (sy1 - sy0) as u32);
        if rw == 0 || rh == 0 {
            return Err(format!("tile {}:{} got no source pixels", tx, ty));
        }
        if u64::from(rw) * u64::from(rh) > REGION_CAP {
            return Err(format!(
                "region {}×{} under the tile exceeds the cap — overviews in the file are too sparse",
                rw, rh
            ));
        }
        let (sx0, sy0) = (sx0 as u32, sy0 as u32);

        let mut region = vec![0u8; (rw as usize) * (rh as usize) * 4];
        for cy in sy0 / ch..=(sy0 + rh - 1) / ch {
            for cx in sx0 / cw..=(sx0 + rw - 1) / cw {
                let (data, dw, dh) = chunks.get(source, image, cy * across + cx)?;
                let (chunk_x, chunk_y) = (cx * cw, cy * ch);
                let ix0 = sx0.max(chunk_x);
                let iy0 = sy0.max(chunk_y);
                let ix1 = (sx0 + rw).min(chunk_x + dw);
                let iy1 = (sy0 + rh).min(chunk_y + dh);
                if ix0 >= ix1 || iy0 >= iy1 {
                    continue;
                }
                let run = ((ix1 - ix0) as usize) * 4;
                for y in iy0..iy1 {
                    let src = (((y - chunk_y) as usize) * (dw as usize) + ((ix0 - chunk_x) as usize)) * 4;
                    let dst = (((y - sy0) as usize) * (rw as usize) + ((ix0 - sx0) as usize)) * 4;
                    region[dst..dst + run].copy_from_slice(&data[src..src + run]);
                }
            }
        }

        let tile = resample_window(&region, rw, rh, window, tw, th);
        emit(level, tx, ty, tw, th, &tile)?;
    }

    veldsdk::log::debug!(target: "perf",
        "pass: level {}, {} tiles, {} chunks ({} from cache), grid {}, {:.2} s",
        level, wants.len(), chunks.asked - chunks.hits, chunks.hits,
        if aligned { "own" } else { "foreign" }, began.elapsed().as_secs_f32());
    Ok(())
}

pub fn pass<S: Chunked>(source: &mut S, grid: &Grid, base: (u32, u32), emit: Emit) -> Result<(), String> {
    let (width, height) = base;
    let (cw, ch) = grid.chunk;
    let across = width.div_ceil(cw);
    let down = height.div_ceil(ch);
    grid.pass_peak(base).admit().map_err(|why| format!("pass: {}", why))?;

    veldsdk::log::debug!(target: "decode",
        "pass: {}×{}, chunk {}×{}, {} rows ({})",
        width, height, cw, ch, down,
        if grid.tiled { "tiled without overviews" } else { "striped" });

    let mut cascade = Cascade::new(0, width, height);
    for cy in 0..down {
        let rows = ch.min(height - cy * ch);
        let mut band = vec![0u8; (width as usize) * (rows as usize) * 4];
        for cx in 0..across {
            let (rgba, dw, dh) = source.chunk(0, cy * across + cx)?;
            let run = (dw as usize) * 4;
            for y in 0..dh.min(rows) as usize {
                let dst = (y * (width as usize) + (cx * cw) as usize) * 4;
                band[dst..dst + run].copy_from_slice(&rgba[y * run..(y + 1) * run]);
            }
        }
        cascade.push_rows(&band, rows, emit)?;
    }
    cascade.finish(emit)
}
