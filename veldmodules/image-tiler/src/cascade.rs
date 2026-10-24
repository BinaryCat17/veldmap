//! Band cascade: every pyramid level in a single pass over the source.
//!
//! The input is full-blooded groups of base-level rows, top to bottom; the
//! output is the ready tiles of every level through a callback, at the moment
//! a level's band is topped up. A band is `TILE` rows of a level: once filled
//! → cut into tiles and halved into the band of the next level. Memory is one
//! band per level, a geometric progression in width; the source height enters
//! only through the level count. [`bytes`] states the exact cost, and it
//! should be asked there rather than derived from this text: the caller holds
//! one more band on top of it, the one it hands to the cascade.
//!
//! Band boundaries are aligned to even rows (TILE is even), and halving goes
//! in exact 2×2 blocks (`resample::halve`), so a bandwise pass yields exactly
//! the same raster as halving the whole level: the content of level k+1 does
//! not know in which portions level k arrived.

use super::pyramid::{self, TILE};
use super::resample::halve;

/// Receiver of ready tiles: level, grid address, actual size, RGBA8.
/// A sink error stops the pass.
pub type Emit<'a> = &'a mut dyn FnMut(u32, u32, u32, u32, u32, &[u8]) -> Result<(), String>;

pub struct Cascade {
    /// Level bands from the base to the top; the vector index is not the
    /// level number: the base need not be zero (JPEG decodes straight at the
    /// scale of the requested level).
    bands: Vec<Band>,
}

struct Band {
    /// Absolute pyramid level number.
    level: u32,
    width: u32,
    height: u32,
    /// Global level row where the current band starts.
    top: u32,
    /// Filled rows in the current band.
    filled: u32,
    buf: Vec<u8>,
}

impl Band {
    fn new(level: u32, width: u32, height: u32) -> Self {
        let rows = height.min(TILE);
        Self {
            level,
            width,
            height,
            top: 0,
            filled: 0,
            buf: vec![0; (width as usize) * (rows as usize) * 4],
        }
    }

    /// Rows in the current band: shorter at the bottom edge.
    fn rows(&self) -> u32 {
        (self.height - self.top).min(TILE)
    }
}

/// What the cascade will cost in instance memory — before it is created.
///
/// This is asked by those deciding whether to take the source at all: the
/// cascade cost adds to the cost of reading itself, and the instance limit is
/// one for both. It is computed here, not by the asker, for the same reason
/// the cascade knows its own levels: an eyeball estimate from the width
/// diverges from the code silently, and a diverged estimate breaks neither
/// the build nor the checks.
///
/// There are two components.
///
/// **Level bands live for the whole pass.** There are exactly as many as
/// levels, and each is `level width × min(level height, TILE) × 4`. Level
/// widths converge geometrically, so the sum is about twice the base band,
/// and it does not depend on the source height at all.
///
/// **Band draining is recursive, and its temporary buffers live together.**
/// A level's `flush` cuts a tile (no more than `TILE × TILE × 4`), halves the
/// band, and goes on with the halved band into the `flush` of the next level —
/// where everything repeats while the previous halved band is still alive. The
/// cost is therefore the sum over all levels, not the worst of them.
pub fn bytes(base_w: u32, base_h: u32) -> u64 {
    bands_bytes(base_w, base_h) + flush_bytes(base_w, base_h)
}

/// Level bands: what lives for the whole pass. Computed by the same walk
/// that builds them, and kept in agreement with the construction.
fn bands_bytes(base_w: u32, base_h: u32) -> u64 {
    levels(base_w, base_h).map(|(w, rows)| u64::from(w) * rows * 4).sum()
}

/// Drain temporaries: the cut tile and the halved band.
///
/// The components are computed differently because they live differently.
/// **The halved bands accumulate across levels**: a level's `flush` goes into
/// the `flush` of the next without releasing its own — at depth k every halved
/// band above is alive. **One tile, the largest, is taken**: it is declared in
/// the tile loop body, dies on every iteration, and is dead by the time of
/// halving, so two tiles are never alive at once.
fn flush_bytes(base_w: u32, base_h: u32) -> u64 {
    let halves: u64 = levels(base_w, base_h)
        .map(|(w, rows)| u64::from(w.div_ceil(2)) * rows.div_ceil(2) * 4)
        .sum();
    let tile = levels(base_w, base_h)
        .map(|(w, rows)| u64::from(w.min(TILE)) * rows * 4)
        .max()
        .unwrap_or(0);
    halves + tile
}

/// Cascade levels as "width, rows in band" pairs — by the same halving that
/// [`Cascade::new`] builds them with.
fn levels(base_w: u32, base_h: u32) -> impl Iterator<Item = (u32, u64)> {
    let (mut w, mut h) = (base_w, base_h);
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let item = (w, u64::from(h.min(TILE)));
        done = w.max(h) <= TILE;
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        Some(item)
    })
}

impl Cascade {
    /// Bands for levels `base_level..`, down to the top (biggest side ≤ the
    /// tile). The base sizes are the raster sizes at `base_level`, and the
    /// halving rule is the same as in `pyramid::level_size` — cascade levels
    /// match the pyramid levels by construction.
    pub fn new(base_level: u32, base_w: u32, base_h: u32) -> Self {
        let mut bands = Vec::new();
        let (mut w, mut h) = (base_w, base_h);
        let mut level = base_level;
        loop {
            bands.push(Band::new(level, w, h));
            if w.max(h) <= TILE {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            level += 1;
        }
        Self { bands }
    }

    /// A group of full base-level rows, top to bottom, RGBA8 contiguously.
    /// The group height is arbitrary — band boundary division happens here.
    pub fn push_rows(&mut self, rows: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        self.feed(0, rows, nrows, emit)
    }

    /// End of source: drain the incomplete bands of all levels. Top-to-bottom
    /// order is mandatory — an incomplete band of level k carries rows into
    /// k+1.
    pub fn finish(mut self, emit: Emit) -> Result<(), String> {
        self.drain(emit)
    }

    /// The body of [`Self::finish`] by reference — so that after draining the
    /// cascade can still be asked what it cost.
    fn drain(&mut self, emit: Emit) -> Result<(), String> {
        for i in 0..self.bands.len() {
            self.flush(i, emit)?;
        }
        Ok(())
    }

    fn feed(&mut self, i: usize, data: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        let width = self.bands[i].width as usize;
        let mut fed = 0u32;
        while fed < nrows {
            let band = &mut self.bands[i];
            let take = (band.rows() - band.filled).min(nrows - fed);
            // The band is at the bottom edge and there is nothing more to
            // take: the source gave more rows than it declared. An error, not
            // a skip: silently dropped rows shift everything below, and a
            // zero take here would loop the pass forever.
            if take == 0 {
                return Err(format!(
                    "level {}: more rows than the declared height {}",
                    band.level, band.height
                ));
            }
            let src = &data[(fed as usize) * width * 4..][..(take as usize) * width * 4];
            let at = (band.filled as usize) * width * 4;
            band.buf[at..at + src.len()].copy_from_slice(src);
            band.filled += take;
            fed += take;
            if band.filled == band.rows() {
                self.flush(i, emit)?;
            }
        }
        Ok(())
    }

    /// Cuts the filled part of the band into tiles and halves it into the
    /// band of the next level.
    fn flush(&mut self, i: usize, emit: Emit) -> Result<(), String> {
        let (level, width, top, rows) = {
            let band = &self.bands[i];
            (band.level, band.width, band.top, band.filled)
        };
        if rows == 0 {
            return Ok(());
        }

        let ty = top / TILE;
        for tx in 0..pyramid::grid(width) {
            let tw = pyramid::tile_extent(tx, width);
            let taken = u64::from(tw) * u64::from(rows) * 4;
            let mut tile = Vec::with_capacity(taken as usize);
            let buf = &self.bands[i].buf;
            for row in 0..rows as usize {
                let from = (row * (width as usize) + (tx * TILE) as usize) * 4;
                tile.extend_from_slice(&buf[from..from + (tw as usize) * 4]);
            }
            emit(level, tx, ty, tw, rows, &tile)?;
            drop(tile);
        }

        let half = if i + 1 < self.bands.len() {
            let band = &self.bands[i];
            let filled = &band.buf[..(width as usize) * (rows as usize) * 4];
            // Level widths are kept consistent by one halving formula.
            debug_assert_eq!(width.div_ceil(2), self.bands[i + 1].width);
            Some((halve(filled, width, rows), rows.div_ceil(2)))
        } else {
            None
        };

        {
            let band = &mut self.bands[i];
            band.top += rows;
            band.filled = 0;
        }
        if let Some((half, hrows)) = half {
            let outcome = self.feed(i + 1, &half, hrows, emit);
            drop(half);
            outcome?;
        }
        Ok(())
    }
}
