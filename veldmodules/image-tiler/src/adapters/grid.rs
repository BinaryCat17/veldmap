//! Chunk grid driver: shared by any source that hands out a raster in chunks
//! on a grid — the base image with its overviews, the chunk being the smallest
//! readable unit. Two loops: pointwise ([`direct`]) builds a level tile from
//! chunks of the nearest usable overview, the pass ([`pass`]) feeds rows of
//! base-image chunks into the cascade as bands. What a level gets is decided
//! by [`Grid::footprint`] from the grid geometry and the chunk cache; the
//! level table (`adapters::table`) and production both read that answer. The
//! format sits behind the [`Chunked`] trait — today TIFF (`tiff::Chunks`); the
//! driver only needs the chunk in RGBA.

use std::collections::VecDeque;

use super::super::budget::{self, Peak};
use super::super::cascade::{self, Cascade, Emit};
use super::super::pyramid::{self, TILE};
use super::super::resample::{resample_window, Window};

/// Cache of decoded chunks for pointwise reads — in `usize` to count cache
/// bytes; the share itself is set by the budget (`budget::CHUNK_CACHE`).
pub(super) const CHUNK_CACHE_BYTES: usize = budget::CHUNK_CACHE as usize;

/// Cap on the source region read for a single tile (in pixels): in RGBA this
/// is half of the chunk cache. The other half covers the chunks around the
/// region: with a foreign grid the bounds spread outward, and twice as many
/// chunks per side can be touched as lie under the region itself.
///
/// A region can exceed the cap when the nearest usable overview is larger
/// than the level by more than the factor of how many tiles fit along the cap
/// side: refusing is more honest than silently reading half the file. Usable
/// means on both sides (see [`Grid::source_for`]), so there are two ways here:
/// a gap of four levels in the overview chain, and an elongated raster whose
/// level short side is one or two pixels — there every overview is dropped,
/// and the level is read from the base image.
pub(super) const REGION_CAP: u64 = budget::CHUNK_CACHE / 2 / 4;

/// Degenerate grid: a chunk smaller than this is read only by the pass.
///
/// Pointwise reading pays for each chunk separately — a trip for it and a
/// decoder launch — and for a chunk of a few dozen pixels that cost exceeds
/// the pixels themselves: a tile built from such chunks takes thousands of
/// calls where the pass unpacks them all in one go. This is how the
/// Sentinel-2 quicklook sits (PVI: 8×8 codeblock tiles); a one-row TIFF strip
/// never lands here — it spans the full raster width.
pub(super) const MIN_CHUNK_PIXELS: u64 = 32 * 32;

/// Cost of a level tile under pointwise reading: the overview it comes from,
/// the overview pixels under the region itself, and the pixels unpacked for
/// it (the region grows to chunk boundaries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub image: usize,
    pub region: u64,
    pub held: u64,
}

/// Reduced copy of the raster — an image with its own chunk grid.
pub struct Overview {
    /// Image index at the source: for TIFF the IFD used by `seek_to_image`.
    pub image: usize,
    pub width: u32,
    pub height: u32,
    /// Its own, not the base one: an overview may be laid out differently from
    /// the base image, and the one chosen by [`Grid::source_for`] is what gets
    /// read.
    pub chunk: (u32, u32),
}

/// Chunk layout of a source: the base image and its overviews.
pub struct Grid {
    pub tiled: bool,
    /// Chunk grid of the base image in pixels: for a tiled file this is the
    /// tile, for a striped one the full raster width by `RowsPerStrip` rows.
    ///
    /// It is kept alongside the description because the smallest readable
    /// unit of the file is the chunk, and a window into half of it costs
    /// exactly as much as the whole. Without this number [`Grid::pointwise`]
    /// would measure the region, while chunks are what is paid for — and in a
    /// striped file, where the chunk spans the full width, the gap between
    /// these two measures is the whole raster.
    pub chunk: (u32, u32),
    /// Overviews in discovery order. In a multi-page TIFF the following IFDs
    /// are separate pages; they are not taken here — they must not replace the
    /// first one.
    pub overviews: Vec<Overview>,
    /// Bytes per pixel in the file's raw samples (channels × sample bytes):
    /// that is what a decoded chunk holds before expansion to RGBA, and
    /// without this number the memory count would treat u16 and f32 rasters as
    /// byte ones.
    pub depth: u32,
}

impl Grid {
    /// An arbitrary tile is cheap: chunks are read pointwise, and overviews
    /// cover the deep levels. For a tiled file without overviews the top tile
    /// would cost reading the whole raster — it goes down the sequential path.
    ///
    /// This is about the source as a whole, not about a level: even here
    /// overviews can be sparse, and a level that got none of its own is read
    /// by the pass (see [`Grid::pointwise`]).
    pub fn random_access(&self) -> bool {
        self.tiled && !self.overviews.is_empty()
    }

    /// Chunk grid of the overview with this image index; zero is the base
    /// raster.
    pub fn chunk_of(&self, image: usize) -> (u32, u32) {
        match self.overviews.iter().find(|overview| overview.image == image) {
            Some(overview) => overview.chunk,
            None => self.chunk,
        }
    }

    /// Source for a level: the smallest overview that covers both of its
    /// sides; `base` holds the base image sizes. Returns the image index and
    /// its sides.
    ///
    /// Covers — up to rounding. A level side is computed by rounding up
    /// (`pyramid::level_size`), while overviews in the file are stored by
    /// rounding down, and for an odd side these two counts differ by exactly
    /// one pixel: for a raster of 25437, level 1 needs 12719 while its
    /// overview in the file is 12718. Requiring an overview no narrower than
    /// the level would reject it and take one twice as large, i.e. read four
    /// times as many pixels per tile.
    ///
    /// The forgiven pixel is a one-column overdraw across the whole tile
    /// (`resample_window` stretches the window wider than it is, by the factor
    /// `level side / overview side`). More than one pixel must not be
    /// forgiven: on a short side a pixel is already whole times over, and a
    /// 513×3 raster would build a level of height 2 from an overview of
    /// height 1.
    ///
    /// Only the rounding is forgiven, so the two sides are checked separately,
    /// and an overview below half the level never qualifies. The second
    /// condition is what guards the short side: for a side of two pixels the
    /// "pixel of difference" is half of it, and such an overview is not
    /// rounded but lost. It can only fire on a side of one or two pixels,
    /// because `2·(n−1) > n` for every `n` above two; it also cuts off the
    /// overview under level zero — there is nothing to round there.
    pub fn source_for(&self, base: (u32, u32), level_width: u32, level_height: u32) -> (usize, u32, u32) {
        // Subtraction from the level side, not addition to the overview side:
        // a corrupt catalog with a width at the u32 ceiling would overflow it
        // along with the assembly.
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

    /// A tile of this level is read as a window — meaning something that
    /// really is a window; `base` holds the base image sizes. What it costs is
    /// [`Grid::footprint`].
    pub fn pointwise(&self, base: (u32, u32), level: u32) -> bool {
        self.footprint(base, level).is_some()
    }

    /// Cost of a level tile under pointwise reading; `None` means the level is
    /// not read pointwise, the pass is what remains for it. Three rules, and
    /// all three are about HOW MUCH MUST BE HELD and WHAT IS PAID FOR to
    /// deliver one tile.
    ///
    /// The grid is not degenerate: the chunk is at least [`MIN_CHUNK_PIXELS`].
    ///
    /// The region is at most [`REGION_CAP`]. It is computed for the WORST tile
    /// of the grid, not the ideal one: bounds spread outward, and the real
    /// region can be a pixel wider (see [`region_of`]). Claiming "window"
    /// where [`direct`] would refuse is doubly wrong: a refusal kills the whole
    /// pass, and the pass condemns all of its cells at once — and the level
    /// will never appear.
    ///
    /// The touched chunks fit in the cache. Not the region area is measured,
    /// but the chunks it touches: a chunk is unpacked whole, and a window into
    /// half of it costs the same. In a striped source the chunk is rows across
    /// the full raster width: the tile needs every strip covering its rows,
    /// and the tile's narrowness saves nothing. If they fit in the cache, the
    /// neighboring tile of the same row takes them ready-made; if not, it
    /// re-reads them from scratch, and a row of seven tiles costs seven file
    /// reads instead of one. One rule covers both branches: a tiled file with
    /// a large tile behaves exactly the same.
    pub fn footprint(&self, base: (u32, u32), level: u32) -> Option<Footprint> {
        let lw = pyramid::level_size(base.0, level).max(1);
        let lh = pyramid::level_size(base.1, level).max(1);
        let (image, sw, sh) = self.source_for(base, lw, lh);
        let (cw, ch) = self.chunk_of(image);
        if u64::from(cw) * u64::from(ch) < MIN_CHUNK_PIXELS {
            return None;
        }
        let aligned = lw.abs_diff(sw) <= 1 && lh.abs_diff(sh) <= 1;

        // With our own grid the region is exactly the tile; with a rescaled one
        // it can be a pixel wider, because floor and ceil spread the bounds in
        // opposite directions.
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

    /// A fresh chunk at the moment of expansion: raw samples and RGBA live
    /// side by side.
    pub fn chunk_bytes(&self, chunk: (u32, u32)) -> u64 {
        u64::from(chunk.0) * u64::from(chunk.1) * (u64::from(self.depth) + 4)
    }

    /// Memory peak of pointwise reading of a level: the chunk cache (the fresh
    /// chunk is placed before eviction, so beyond the cache there is one more
    /// chunk), the region of the worst tile, and the tile itself with its
    /// compression. `None` means the level is not read pointwise.
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

    /// Memory peak of the pass: a row of base-image chunks as a band, the
    /// fresh chunk, and the cascade bands. A striped file with a chunk across
    /// the whole scene asks here for a copy of the entire raster — and gets a
    /// refusal by the count, not by a special cap.
    pub fn pass_peak(&self, base: (u32, u32)) -> Peak {
        let (width, height) = base;
        Peak::new()
            .with("band", u64::from(width) * u64::from(self.chunk.1) * 4)
            .with("fresh chunk", self.chunk_bytes(self.chunk))
            .with("cascade", cascade::bytes(width, height))
    }
}

/// How many pixels of a side are actually unpacked under a region of `side`
/// pixels with a chunk of `chunk` and an overview of `source`.
///
/// The region grows to the chunk grid bounds, and which tile is asked for is
/// not known in advance: [`Grid::pointwise`] answers for the level as a whole.
/// So the worst offset among those this level can have is taken — and they do
/// differ, depending on whether the level grid matches the overview grid
/// (`aligned`). It never exceeds what the overview has: the number of chunks
/// is finite.
pub(super) fn spanned(side: u64, chunk: u32, source: u32, aligned: bool) -> u64 {
    if side == 0 {
        return 0;
    }
    let chunk = u64::from(chunk.max(1));
    let tile = u64::from(TILE);
    // Everything hinges on how far from a chunk boundary the region can start.
    // With our own grid the start is a multiple of the tile (see
    // [`region_of`]), and if the grids are commensurate — the chunk is a
    // multiple of the tile or vice versa — there are finitely many such
    // starts, the farthest from the boundary standing at `chunk − tile`. With
    // a foreign grid the start is arbitrary, and the worst stands a whole
    // chunk minus one away.
    //
    // The difference is not subtle: for a tiled COG with an inner tile of 4096
    // our own grid puts the whole screen tile into one chunk, while the foreign
    // measure would count two along each side — that is four times more memory
    // than needed, and the level would be declared unfit for a window for no
    // reason.
    let flush = aligned && (chunk % tile == 0 || tile % chunk == 0);
    let offset = match flush {
        true => chunk - tile.min(chunk),
        false => chunk - 1,
    };
    let touched = ((offset + side - 1) / chunk + 1).min(u64::from(source).div_ceil(chunk));
    touched.saturating_mul(chunk)
}

/// Overview region under one level tile.
pub(super) struct Region {
    pub(super) sx0: u64,
    pub(super) sx1: u64,
    pub(super) sy0: u64,
    pub(super) sy1: u64,
    /// The share of the read area that the tile itself occupies. The read area
    /// can be wider by a fraction of a pixel on each side — averaging needs
    /// every touched pixel — and it is the window that gets compressed: the
    /// read area stretched over the tile would shift entirely by that
    /// fraction, the neighboring tile's the other way, and a seam would remain
    /// at the joint.
    pub(super) window: Window,
}

/// Where to look for a tile in the overview. A pure function, because what
/// depends on it is not the look of the picture but the cost of reading: a
/// pixel attributed to the region beyond what is needed turns into extra file
/// chunks and jumps over them (see `aligned` in [`direct`]).
///
/// `aligned` — the overview and the level are one and the same grid. Then the
/// tile is taken exactly where it lies, and the window matches what is read.
/// Otherwise the scale is fractional and the bounds spread outward
/// (floor/ceil): for power-of-two overviews the fractions are zero and there
/// is no difference, while non-binary ones (3, 5, 7/2) produce it on every
/// tile.
pub(super) fn region_of(
    (tx, ty): (u32, u32),
    (tw, th): (u32, u32),
    (lw, lh): (u32, u32),
    (sw, sh): (u32, u32),
    aligned: bool,
) -> Region {
    let (x0, y0) = (u64::from(tx) * u64::from(TILE), u64::from(ty) * u64::from(TILE));
    if aligned {
        // The start steps back just far enough for the tile to get at least one
        // row of the overview. The overview is shorter than the level by that
        // very forgiven pixel, and the last tile may have none of it left at
        // all: a side of 4609 is ten rows, the last one a single pixel, while
        // the overview ends at 4608. An empty region here is not an empty tile
        // but a refusal of the whole pass — and with it a verdict against all
        // of its cells at once (see `Fetch::produced`), meaning a row that
        // will never appear.
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

/// A source that serves a raster in chunks on a grid ([`Grid`]).
pub trait Chunked {
    /// Chunk `index` of image `image` — RGBA8 and its actual size: for edge
    /// chunks the useful part is shorter than the grid. The index is
    /// `row × chunks per row + column`.
    fn chunk(&mut self, image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String>;

    /// Chunks `indices` of image `image` are about to be read — a source that
    /// knows where they lie in the file tells the medium so (`veldsdk::abi::
    /// resource_prefetch`), and they travel over the wire together, with
    /// several requests in flight, rather than miss by miss. The default is
    /// nothing: not every source knows its offsets.
    fn prefetch(&mut self, _image: usize, _indices: &[u32]) -> Result<(), String> {
        Ok(())
    }
}

/// Decoded chunks of one image, RGBA8. Eviction is by seniority and up to the
/// budget: tiles arrive in neighborhood order, and the oldest is the least
/// needed one.
struct ChunkCache {
    entries: VecDeque<(u32, Vec<u8>, u32, u32)>,
    bytes: usize,
    /// Accesses and, of those, hits. Counted because this pair reveals the
    /// main cost of the pass: a chunk taken ready-made costs nothing, while a
    /// miss is a trip to the network, and a single one costs more than all the
    /// unpacking of the order together. A grid drifted apart from the level
    /// shows up right here too — by a miss count four times the tile count.
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

        // The fresh one survives any budget: without it the current tile
        // cannot be assembled.
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

/// Pointwise reading: tiles `wants` of level `level` from chunks of the
/// nearest usable overview; `base` holds the base image sizes.
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

    // The overview and the level are one and the same grid if they differ by
    // no more than a rounding pixel. [`Grid::source_for`] declared exactly
    // this forgiveness when CHOOSING the overview — and missing it when
    // READING is expensive: the level side is computed by rounding up, the
    // overview in the file is stored by rounding down, and the proportional
    // rescale shifts each tile's region by a pixel to the left and up. The
    // region comes out a pixel wider than the tile and touches four times as
    // many file chunks. By itself this is cheap — the extra chunk usually
    // comes ready-made from the cache — but the expensive part is elsewhere:
    // chunks are assembled in rows, a row stands a whole grid width away from
    // its neighbor, and the reading walks the file back and forth on every
    // tile. The host reads such walking as random access and resets the
    // readahead ramp-up to a single block (see `Readahead` in
    // network/range.rs), dropping all the speed with it.
    //
    // What is paid for this is a pixel of content at the far edge of the tile:
    // today the same fraction of a pixel is smeared by the resample across the
    // whole side. The georeference does not move meanwhile — it is computed
    // from the level grid, not from the overview.
    let aligned = lw.abs_diff(sw) <= 1 && lh.abs_diff(sh) <= 1;
    let began = std::time::Instant::now();

    veldsdk::log::debug!(target: "decode",
        "pointwise access: level {} ({}×{}) from image {} ({}×{}), {} tiles, grid {}",
        level, lw, lh, image, sw, sh, wants.len(),
        if aligned { "own" } else { "foreign" });

    // Chunks under all ordered tiles go to the medium up front, in one order
    // (see [`Chunked::prefetch`]); the same geometry as the assembly below.
    // A failed order does not kill the pass: the chunks arrive by ordinary
    // reading.
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

        // The tile rectangle in source pixels. The scale is fractional, the
        // bounds go outward (floor/ceil): averaging needs every touched pixel.
        //
        // What is read is WIDER than what belongs to the tile — by a fraction
        // of a pixel on each side. The tile owns the `window`, and it is the
        // window that gets compressed: the read area stretched over the tile
        // would shift entirely by that fraction, the neighboring tile's the
        // other way, and a seam would remain at the joint. For power-of-two
        // overviews the fractions are zero and there is no difference, while
        // non-binary ones (3, 5, 7/2) produce it on every tile.
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

        // The region is assembled from the intersection with the chunks;
        // beyond the data edge there is nothing — for edge chunks the useful
        // part is shorter.
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

    // What the pass cost. The line is here because the tile cost cannot be
    // measured otherwise: only "how much was read" travels outward, and that
    // is the watermark of the read head, which under random access stands
    // still while the reading walks backward.
    //
    // The essence of it is chunk-cache misses against tiles. They are even
    // when the grid is our own. More — when the region has drifted from the
    // overview grid, and what grows then is not so much the miss count as the
    // time: the reading walks the file backward, and the host reads the
    // backward jump as random access. Misses grow fourfold for an overview
    // twice the size of the level (the file may lack one at this step
    // entirely) — there it is the honest price, not a grid drift.
    veldsdk::log::debug!(target: "perf",
        "pass: level {}, {} tiles, {} chunks ({} from cache), grid {}, {:.2} s",
        level, wants.len(), chunks.asked - chunks.hits, chunks.hits,
        if aligned { "own" } else { "foreign" }, began.elapsed().as_secs_f32());
    Ok(())
}

/// Sequential pass: rows of base-image chunks from top to bottom — as bands
/// into the cascade that builds all levels at once; `base` holds the base
/// image sizes.
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
        // The row height follows the grid: at the bottom edge the row is
        // shorter as a whole.
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
