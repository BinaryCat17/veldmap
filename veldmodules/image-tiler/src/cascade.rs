use super::pyramid::{self, TILE};
use super::resample::halve;

pub type Emit<'a> = &'a mut dyn FnMut(u32, u32, u32, u32, u32, &[u8]) -> Result<(), String>;

pub struct Cascade {
    bands: Vec<Band>,
}

struct Band {
    level: u32,
    width: u32,
    height: u32,
    top: u32,
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

    fn rows(&self) -> u32 {
        (self.height - self.top).min(TILE)
    }
}

pub fn bytes(base_w: u32, base_h: u32) -> u64 {
    bands_bytes(base_w, base_h) + flush_bytes(base_w, base_h)
}

fn bands_bytes(base_w: u32, base_h: u32) -> u64 {
    levels(base_w, base_h).map(|(w, rows)| u64::from(w) * rows * 4).sum()
}

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

    pub fn push_rows(&mut self, rows: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        self.feed(0, rows, nrows, emit)
    }

    pub fn finish(mut self, emit: Emit) -> Result<(), String> {
        self.drain(emit)
    }

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
