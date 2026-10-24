//! Source level table: how each level is served, what it costs, and whether
//! it fits in memory. One table for both description and production: it
//! travels over the wire as rows (`Described.levels`), and `adapters::produce`
//! picks the arm by it — the rule is written down once.
//!
//! Rows are computed from the header, without pixels: for TIFF, JPEG 2000 and
//! NetCDF from the chunk grid (`Grid::footprint`), for the rest from the frame
//! and their decoder rules. The peak is `budget::Peak`; the "fits" column is
//! its check against the free memory along with the path ceiling, if the path
//! has one of its own: `FULL_DECODE_BUDGET` for a whole frame.

use super::super::budget::Peak;
use super::super::cascade;
use super::super::pyramid;
use super::grid::Overview;
use super::{frame_fits, jpeg, Info, Kind, Tie};

/// How a level is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    /// Pointwise: only the chunks under the requested tiles are read.
    Pointwise,
    /// With a pass that starts at level `from` and builds it and every coarser
    /// level via the cascade. Zero — the whole pyramid in one pass; the level
    /// itself — the decoder delivers the frame straight at its scale, and
    /// finer detail comes in a new pass.
    Pass { from: u32 },
}

/// A table row — one pyramid level.
#[derive(Clone, Debug)]
pub struct Level {
    pub serve: Serve,
    /// Reading cost: source pixels decoded for a single tile (pointwise) or
    /// for the whole pass (the entire raster).
    pub pixels: u64,
    /// Memory peak of the work at this level — as named components.
    pub peak: Peak,
    /// Whether the work fits in memory: the peak against free memory along
    /// with the path ceiling, if the path has one of its own.
    pub fits: bool,
}

impl Info {
    /// The level table, from level zero to the top.
    pub fn levels(&self) -> Vec<Level> {
        let base = (self.width, self.height);
        let whole = u64::from(self.width) * u64::from(self.height);
        let pass = |peak: Peak, fits: bool| Level { serve: Serve::Pass { from: 0 }, pixels: whole, fits, peak };
        (0..pyramid::level_count(self.width, self.height))
            .map(|level| {
                let lw = pyramid::level_size(self.width, level);
                let lh = pyramid::level_size(self.height, level);
                // Chunk grid — pointwise where the window is a real window,
                // otherwise with a pass.
                let chunked = |grid: &super::grid::Grid| match grid.footprint(base, level) {
                    Some(footprint) => {
                        let peak = grid.direct_peak(base, level).expect("pointwise level");
                        Level { serve: Serve::Pointwise, pixels: footprint.held, fits: peak.fits(), peak }
                    }
                    None => {
                        let peak = grid.pass_peak(base);
                        pass(peak.clone(), peak.fits())
                    }
                };
                match &self.kind {
                    Kind::Tiff(layout) => chunked(&layout.grid),
                    Kind::Jp2(layout) => chunked(&layout.grid),
                    // Full-width row windows — a grid without copies, like a stripped TIFF.
                    Kind::Netcdf(layout) => chunked(&layout.grid),
                    // Streaming PNG: a row and the cascade bands.
                    Kind::Png { interlaced: false } => {
                        let peak = Peak::new()
                            .with("row", u64::from(self.width) * 4)
                            .with("cascade", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), peak.fits())
                    }
                    // Whole frame: decoded and again in RGBA.
                    Kind::Png { interlaced: true } | Kind::Full(_) => {
                        let peak = Peak::new()
                            .with("frame", whole * 4 * 2)
                            .with("cascade", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), frame_fits(self.width, self.height) && peak.fits())
                    }
                    // Frame at the decoder scale, brought to the level grid.
                    Kind::Jpeg => {
                        let (dw, dh) = jpeg::decoded_size(self.width, self.height, level);
                        let decoded = u64::from(dw) * u64::from(dh);
                        let peak = Peak::new()
                            .with("raw frame", decoded * 3)
                            .with("frame", decoded * 4)
                            .with("level", u64::from(lw) * u64::from(lh) * 4)
                            .with("cascade", cascade::bytes(lw, lh));
                        Level {
                            serve: Serve::Pass { from: level },
                            pixels: whole,
                            fits: frame_fits(dw, dh) && peak.fits(),
                            peak,
                        }
                    }
                }
            })
            .collect()
    }

    /// The row of a level; `None` — the pyramid has no such level.
    pub fn level(&self, level: u32) -> Option<Level> {
        self.levels().into_iter().nth(level as usize)
    }

    /// How many levels from zero are read pointwise — the length of the
    /// pointwise prefix of the table.
    pub fn windowed(&self) -> u32 {
        self.levels().iter().take_while(|row| row.serve == Serve::Pointwise).count() as u32
    }

    /// Detail limit: the first level that fits. If none fits — the top; such
    /// a source is not described (`adapters::checked`), and the answer here is
    /// for a table computed outside the description. The consumer computes the
    /// same from the wire rows (`tiles::Meta::finest`).
    pub fn finest(&self) -> u32 {
        let rows = self.levels();
        let top = rows.len().saturating_sub(1) as u32;
        rows.iter().position(|row| row.fits).map_or(top, |at| at as u32)
    }

    /// How much memory the parsing itself holds while it sits in the memo:
    /// tie points and the copy grid. A component of the neighbouring work
    /// peak (`module::State::neighbour_footprint`).
    pub fn footprint(&self) -> u64 {
        let ties = (self.ties.len() * std::mem::size_of::<Tie>()) as u64;
        let kind = match &self.kind {
            Kind::Tiff(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Jp2(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Netcdf(_) | Kind::Png { .. } | Kind::Jpeg | Kind::Full(_) => 0,
        };
        ties + kind
    }
}
