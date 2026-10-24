//! TIFF: the only format with cheap random access — and the one that can be
//! the most expensive, when the layout is striped and has no overviews.
//!
//! Both paths — pointwise (a tiled file with copies, COG) and pass (strips or
//! tiles without copies) — belong to the chunk grid driver (`grid.rs`); here
//! the format contributes only the chunk: the `tiff` crate decoder aimed at
//! an image, and the file stretch ([`Chunks`]).
//!
//! Samples wider than a byte (u16, i16, f32 — radar, DEM) go to RGBA through
//! a stretch over a file sample, "no data" as transparency; the rules live in
//! radiometry.rs, here only the sample choice (see [`mapping`]).

use std::cell::RefCell;
use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use super::super::budget::Peak;
use super::super::cascade::Emit;
use super::grid::{self, Chunked, Grid, Overview};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples};
use super::{placed, Info, Kind, Placement, Tie};

/// BigTIFF signatures: its header carries version 43 instead of 42, and that
/// number is how it is recognized. It is checked here, next to JP2 and
/// NetCDF, because the `image` crate knows only the classic signatures and
/// answers "this is not an image" — while the `tiff` crate reads such a file
/// like any other.
pub const BIG_MAGIC: [&[u8]; 2] = [b"II\x2b\x00", b"MM\x00\x2b"];

pub struct Layout {
    /// Chunk layout — what the driver operates on.
    pub grid: Grid,
/// Display stretch, computed on first need and ready afterwards.
    ///
    /// It is a property of the file, not of the request: `Mapping` explicitly
    /// promises "once per file", otherwise neighboring tiles would come out
    /// with visible seams. A recomputation costs four scattered reads over the
    /// file and a sort of up to a million samples, while a single file gets
    /// dozens of requests — one per ladder step and for every edge that comes
    /// into view. Those reads also walk the file backwards, and the host reads
    /// such a walk as random access, dropping the readahead ramp-up right
    /// before the work itself.
    ///
    /// Interior mutability because the consumer holds the parsed description
    /// by reference, while the stretch can only be computed with a reader in
    /// hand — that is, already inside production. The instance is
    /// single-threaded, and `RefCell` costs the same here as an ordinary field.
    stretch: RefCell<Option<Mapping>>,
}

impl Layout {
    /// Layout without a computed stretch — it is computed on first need;
    /// `depth` is bytes per pixel in the file's raw samples.
    pub fn of(tiled: bool, chunk: (u32, u32), overviews: Vec<Overview>, depth: u32) -> Self {
        Self { grid: Grid { tiled, chunk, overviews, depth }, stretch: RefCell::new(None) }
    }

    /// The IFD the stretch sample is taken from: the smallest copy, or the
    /// base raster when there are no copies. It is cheap and one for all
    /// levels.
    ///
    /// A property of the file, not of the request, and both branches ask for
    /// it the same way. The file has a single stretch (see
    /// [`Layout::stretch`]): computed sometimes from a tiny copy, sometimes
    /// from four chunks of the base, it would give neighboring tiles of one
    /// level different brightness — and which of them land in the disk cache
    /// would depend on request order.
    fn stats(&self) -> usize {
        self.grid.overviews.iter().min_by_key(|overview| overview.width).map_or(0, |o| o.image)
    }
}

pub fn describe<R: Read + Seek>(reader: R) -> Result<Info, String> {
    let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
    let (width, height) = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;
    ensure_chunky(&mut decoder)?;
    ensure_readable(&mut decoder)?;
    let tiled = decoder.get_tag_unsigned::<u32>(Tag::TileWidth).is_ok();
    let depth = depth_of(&mut decoder)?;
    // Not a refusal but the worst case: a chunk that cannot be measured or
    // does not fit in memory is "the whole raster", and `Grid::footprint`
    // gives such a level no window. Whether at least one level then fits as a
    // pass is decided by the level table at the general description check
    // (`adapters::checked`).
    let chunk = chunk_grid(&mut decoder).unwrap_or((width, height));
    let (ties, placement, binding_trouble) = georef(&mut decoder, width, height);

    let mut overviews = Vec::new();
    let mut index = 0;
    while decoder.more_images() {
        if decoder.next_image().is_err() {
            break;
        }
        index += 1;
        let subfile = decoder.get_tag_unsigned::<u32>(Tag::NewSubfileType).unwrap_or(0);
        if !is_overview(subfile) {
            continue;
        }
        let Ok((w, h)) = decoder.dimensions() else { continue };
        if w == 0 || h == 0 {
            continue;
        }
        // Same rule as for the base: an unmeasurable chunk means the whole
        // copy. `Grid::source_for` is free to select it, but it gives no
        // window, and the level goes as a pass.
        let chunk = chunk_grid(&mut decoder).unwrap_or((w, h));
        overviews.push(Overview { image: index, width: w, height: h, chunk });
    }

    Ok(Info {
        width,
        height,
        kind: Kind::Tiff(Layout::of(tiled, chunk, overviews, depth)),
        ties,
        placement,
        // Only Sentinel-3 declares an instrument reading in its global
        // attributes; for GeoTIFF the place is written by the raster itself.
        frame: None,
        binding_trouble,
        variable: None,
        variables: Vec::new(),
    })
}

/// Whether this is a shrunk copy of the picture — by the NewSubfileType tag.
///
/// "Reduced" (bit 1) alone is not enough: an alpha mask is marked with bit 4,
/// and its own copies carry 5 = 1|4 — that is how GDAL writes them, and such a
/// copy would turn out to be a one-bit mask instead of a scene. The pyramid
/// takes a copy by size, so a foreign one among its own is not rejected
/// downstream — it gets shown.
fn is_overview(subfile_type: u32) -> bool {
    const REDUCED: u32 = 1;
    const MASK: u32 = 4;
    subfile_type & REDUCED != 0 && subfile_type & MASK == 0
}

/// GeoTIFF georeference: nodes in degrees, or a binding to a projection. One
/// of the two, because only one is recorded in the file — degrees or system
/// meters — and mixing them would be lying with numbers rather than staying
/// silent.
///
/// Degrees are parsed by [`geo_ties`], the projection by [`geo_placement`];
/// here only tag reading and what must be said aloud.
///
/// Third goes the caveat — the one that reaches the viewer
/// ([`Info::binding_trouble`]): an empty binding alone gives nothing to
/// explain.
///
/// The decoder stands on the same image after the return: there is no aiming
/// here.
fn georef<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    width: u32,
    height: u32,
) -> (Vec<Tie>, Option<Placement>, Option<String>) {
    let keys = decoder.get_tag_u16_vec(Tag::GeoKeyDirectoryTag).unwrap_or_default();
    let points = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).unwrap_or_default();
    let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).unwrap_or_default();
    // The crate does not know the matrix tag: it lists only the common ones. The
    // number comes from the GeoTIFF spec — ModelTransformationTag.
    let matrix = decoder.get_tag_f64_vec(Tag::Unknown(34264)).unwrap_or_default();

    // A foreign datum concerns the binding that just took: the numbers in the
    // file are correct, but they land a hundred metres off to the side. For the
    // viewer this is not a caption under the image (the image sits within
    // accuracy enough for the eye), but for the parser it is a line in the log,
    // and it leaves here regardless of who describes the raster and how the
    // binding ends.
    if let Some(code) = foreign_datum(&keys) {
        veldsdk::log::warn!(target: "decode",
            "the raster declares datum EPSG:{}, but the binding lands as WGS84: a shift of about a hundred metres",
            code);
    }
    let placement = geo_placement(&keys, &points, &scale, &matrix, width, height);
    let ties = geo_ties(&keys, &points, &scale, &matrix, width, height);
    // The condition goes by the binding tags, not the coordinate model: silent
    // outcomes are as common for geocentric (1024 = 3) and user-defined as for
    // a projection, and a named kind would leave them all without explanation.
    let carries = !points.is_empty() || !scale.is_empty() || !matrix.is_empty();
    let taken = !ties.is_empty() || placement.is_some();
    let trouble = binding_trouble(carries, taken, &keys, points.len() / 6);
    // Into the log — right here, where the cause is born. As a field it travels
    // to the viewer, but does not always arrive: the consumer carries it and
    // decides by its own layer. The parser needs it in either case.
    if let Some(said) = &trouble {
        veldsdk::log::warn!(target: "decode", "{}", said);
    }
    (ties, placement, trouble)
}

/// A caveat about the binding: the file mentions it, but there is no way to
/// read it.
///
/// This must be said exactly here: downstream, "the file is silent" and "the
/// file speaks, but it did not read" look the same — an empty binding — and
/// there is nothing to explain about such emptiness.
///
/// One kind of statement, and that is the whole point of the field: "there is
/// no binding, and here is why". A foreign datum does not belong here — it is
/// about a binding that was taken — and squeezed in here it would tell the
/// viewer "location unknown" about an image that lies in its place.
///
/// A separate function for the test's sake: this is where what a person reads
/// is decided.
fn binding_trouble(carries: bool, taken: bool, keys: &[u16], points: usize) -> Option<String> {
    (carries && !taken).then(|| {
        // A number, not a debug print: `Some(1)` in a caption under the image
        // tells the viewer nothing, while a number is enough for the parser.
        let named = |id: u16| match geokey(keys, id) {
            Some(code) => code.to_string(),
            None => "unnamed".to_string(),
        };
        format!(
            "the raster carries a binding, but there is no way to read it: coordinate model {}, system {}, tie points {}",
            named(1024), named(3072), points
        )
    })
}

/// The value of a simple geokey: keys lie in fours after a header of four
/// numbers, and a simple key (slot 0) has its value as the fourth in the four.
fn geokey(keys: &[u16], id: u16) -> Option<u16> {
    keys.get(4..)
        .unwrap_or_default()
        .chunks_exact(4)
        .find(|entry| entry[0] == id && entry[1] == 0)
        .map(|entry| entry[3])
}

/// Node shift when the coordinate names the middle of a pixel, not its corner
/// (GTRasterTypeGeoKey, 1025). Outward the node travels as a fraction of the
/// raster, where zero is the edge, so such a node must be shifted by half a
/// pixel. The spec default is the corner, and then there is no shift.
///
/// Half a pixel is no trifle for rasters used for measurement: the Copernicus
/// DSM step is one arc-second, and half of it is fifteen metres on the ground.
fn half_pixel(keys: &[u16]) -> f64 {
    if geokey(keys, 1025) == Some(2) { 0.5 } else { 0.0 }
}

/// The datum declared by the file — if it is not WGS84 (GeographicTypeGeoKey,
/// 2048).
///
/// This is asked because downstream there is no room for a datum: both the
/// nodes and the frame are declared in WGS84, so a Pulkovo-1942 (EPSG:4284)
/// file would drift there silently — and that is a hundred metres in the middle
/// latitudes. The numbers here are not corrected: translating between datums is
/// more geodesy, and it must not be introduced for the sake of one line. The
/// line is the whole answer.
///
/// The file's silence and `user-defined` are not a foreign datum but the
/// absence of an answer: there is nothing to say about them.
fn foreign_datum(keys: &[u16]) -> Option<u16> {
    // 4055 — the "popular visualization sphere", the datum of web Mercator: its
    // coordinates are numerically the same as WGS84's, which is what that
    // projection is built on. Old exports to EPSG:3857 declare it all over, and
    // it does not count as foreign.
    const WGS84: [u16; 3] = [4326, 4979, 4055];
    const UNSAID: [u16; 2] = [0, 32767];
    geokey(keys, 2048).filter(|code| !WGS84.contains(code) && !UNSAID.contains(code))
}

/// Whether the pixel step is usable: both sides are real numbers and non-zero.
/// A zero one would fold the raster into a line, and such a binding is
/// indistinguishable from a real one from the outside — nothing catches it, not
/// by the number of nodes nor by their order.
///
/// Not-a-number is checked separately because comparison with zero lets it
/// through: `NaN != 0` is true.
fn usable_step(scale: &[f64]) -> bool {
    matches!(scale.get(..2), Some([x, y]) if x.is_finite() && y.is_finite() && *x != 0.0 && *y != 0.0)
}

/// The binding of a raster lying in a projection: the EPSG code and the linear
/// transform from pixel to system metres.
///
/// `None` — the file lies in degrees (then [`geo_ties`] reads it), the system
/// is unnamed, named with an unclear code, or the transform is degenerate.
///
/// A grid of tie points means a refusal here, not a binding from the first
/// node: a grid describes a nonlinear layout — that is what makes it a grid —
/// and six numbers cannot express it. Taken from a single node, it would lie
/// the further you go from that node.
///
/// A truncated tie point tag (fewer than six numbers) does not reach here at
/// all and goes into the matrix branch — and having no matrix, it ends in a
/// refusal. Reading half a node and completing the second half would be a
/// guess, not reading.
fn geo_placement(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
    width: u32,
    height: u32,
) -> Option<Placement> {
    // GTModelTypeGeoKey (1024): 1 — projection metres.
    if geokey(keys, 1024) != Some(1) || points.len() > 6 {
        return None;
    }
    // ProjectedCSTypeGeoKey (3072). Zero and user-defined are not codes: the
    // parameters of such a system lie in separate keys, and assembling it from
    // them is projection parsing, not raster reading.
    let epsg = match geokey(keys, 3072) {
        Some(code) if code != 0 && code != 32767 => u32::from(code),
        _ => return None,
    };

    let half = half_pixel(keys);
    let affine = match (points.get(..6), usable_step(scale)) {
        // The Y step is positive while raster rows run south — hence the minus.
        (Some(tie), true) => [
            scale[0],
            0.0,
            tie[3] - (tie[0] + half) * scale[0],
            0.0,
            -scale[1],
            tie[4] + (tie[1] + half) * scale[1],
        ],
        _ => affine_from_matrix(matrix, half)?,
    };
    // The tie point is checked together with everything else: the step could be
    // usable while the place is not a number, and such a frame would reach the
    // globe looking whole.
    //
    // Finite terms are not enough — the far corner is checked too. With large
    // terms the product by the raster side overflows, and the globe's frame
    // measure looks only at the first column (`Frame::ground_m_per_px`) and
    // misses it: the frame comes out measurable while its place is not a number.
    let (right, bottom) = (f64::from(width), f64::from(height));
    let corner = [
        affine[0] * right + affine[1] * bottom + affine[2],
        affine[3] * right + affine[4] * bottom + affine[5],
    ];
    let whole = affine.iter().chain(corner.iter()).all(|value| value.is_finite());
    whole.then_some(Placement { epsg, affine })
}

/// Tie points of a raster lying in degrees. Empty if the file is bound to a
/// projection: only one who knows the projection itself could convert it, and
/// the tiler knows about the raster and not about the Earth (see
/// [`geo_placement`]).
///
/// Geokey parsing is separated from reading because it is checked through them:
/// there is no file with a grid in the tests, but the rules "degrees or
/// projection", "six numbers per node", and "the Y step runs south" are there.
fn geo_ties(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
    width: u32,
    height: u32,
) -> Vec<Tie> {
    // GTModelTypeGeoKey (1024): 2 — degrees.
    if geokey(keys, 1024) != Some(2) {
        return Vec::new();
    }
    let half = half_pixel(keys);

    let point = |px: f64, py: f64, lon: f64, lat: f64| Tie { px, py, lat, lon };
    // A node is six numbers: pixel (i, j, k) and place (x, y, z).
    if points.len() > 6 {
        let ties: Vec<Tie> = points
            .chunks_exact(6)
            .map(|tie| point(tie[0] + half, tie[1] + half, tie[3], tie[4]))
            .collect();
        // The tag's numbers are as written in the file, and they are checked all
        // around: a place that does not exist on Earth cancels the whole grid.
        // The all-around check does not suit the derived corners (below) — for a
        // raster whose anchor sits far from the corner, the corner legitimately
        // goes past the pole.
        return match ties.iter().all(|tie| placed(tie.lat, tie.lon)) {
            true => finite(ties),
            false => Vec::new(),
        };
    }

    // One point with the pixel step: the raster lies in degrees as a plain
    // rectangle, and its corners are enough.
    let (Some(tie), true) = (points.get(..6), usable_step(scale)) else {
        return finite(corners_from_matrix(matrix, half, width, height));
    };
    // The Y step is positive while raster rows run south — hence the minus.
    let (x, y) = (tie[3] - (tie[0] + half) * scale[0], tie[4] + (tie[1] + half) * scale[1]);
    let (right, bottom) = (f64::from(width), f64::from(height));
    finite(vec![
        point(0.0, 0.0, x, y),
        point(right, 0.0, x + right * scale[0], y),
        point(0.0, bottom, x, y - bottom * scale[1]),
        point(right, bottom, x + right * scale[0], y - bottom * scale[1]),
    ])
}

/// Points, every one of them finite — or nothing at all.
///
/// The tag is read from the file as is, and the file arrives from the network:
/// its numbers can be anything, infinity included. One bad point cancels the
/// whole grid for the same reason as with NetCDF: the consumer needs a complete
/// rectangle, and a grid without a node cannot be assembled.
///
/// The check stands here, at the entrance, not at the consumer. An infinite
/// longitude, having reached the globe, drags the rotation into arithmetic that
/// has no answer, and a bad raster fraction spreads from there through the warp
/// mesh and the level measure.
fn finite(ties: Vec<Tie>) -> Vec<Tie> {
    let whole = |tie: &Tie| {
        tie.px.is_finite() && tie.py.is_finite() && tie.lat.is_finite() && tie.lon.is_finite()
    };
    match ties.iter().all(whole) {
        true => ties,
        false => Vec::new(),
    }
}

/// The affine transform from the binding matrix (ModelTransformationTag).
///
/// The matrix is four rows of four numbers, and the place of pixel (i, j) is
/// given by the first pair of rows: `x = a·i + b·j + d`, `y = e·i + f·j + h`.
/// Such a binding stands in place of the "point + step" pair and differs from
/// it in that the raster may lie rotated: the pair has its step along the axes,
/// and there is nothing to rotate it with.
///
/// `None` — a degenerate matrix (a zero row, the identity placeholder). It is
/// not a binding: all four corners would collapse into a point, and the raster
/// would lie into nothing.
///
/// The half-pixel of the middle goes into the constant term rather than being
/// taken off the argument on every call: one transform travels outward from
/// here, and it must not have a second convention.
fn affine_from_matrix(matrix: &[f64], half: f64) -> Option<[f64; 6]> {
    let a = matrix.get(..8)?;
    if !a.iter().all(|value| value.is_finite()) || a[0] * a[5] - a[1] * a[4] == 0.0 {
        return None;
    }
    Some([
        a[0],
        a[1],
        a[3] - (a[0] + a[1]) * half,
        a[4],
        a[5],
        a[7] - (a[4] + a[5]) * half,
    ])
}

/// Corners of a matrix-bound raster — in degrees.
///
/// Corners are enough even for a rotated one: the transform is linear, and
/// between four nodes a grid reconstructs the linear exactly.
fn corners_from_matrix(matrix: &[f64], half: f64, width: u32, height: u32) -> Vec<Tie> {
    let Some(a) = affine_from_matrix(matrix, half) else { return Vec::new() };
    let (right, bottom) = (f64::from(width), f64::from(height));
    let place = |px: f64, py: f64| Tie {
        px,
        py,
        lon: a[0] * px + a[1] * py + a[2],
        lat: a[3] * px + a[4] * py + a[5],
    };
    vec![place(0.0, 0.0), place(right, 0.0), place(0.0, bottom), place(right, bottom)]
}

// ── Direct access ──────────────────────────────────────────────

/// A piece of the copy under one tile: where it lies and which part of it
/// belongs to the tile.
/// TIFF chunks behind the driver trait: the `tiff` crate decoder, the file
/// stretch, and the image the decoder stands on, with its color model.
struct Chunks<R: Read + Seek> {
    decoder: Decoder<R>,
    mapping: Mapping,
    at: Option<(usize, Pixel)>,
    /// The resource the reader works on: chunks are named to it ahead of time.
    resource: u64,
}

impl<R: Read + Seek> Chunks<R> {
    fn new(reader: R, resource: u64, layout: &Layout) -> Result<Self, String> {
        let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
        // For byte files the stretch costs not a single extra read. Where the
        // decoder stays after it is not promised (see [`stretch`]) — the first
        // read aims it itself.
        let mapping = stretch(layout, &mut decoder)?;
        Ok(Self { decoder, mapping, at: None, resource })
    }

    /// Aims the decoder at the image and answers its color model. Copies can be
    /// laid out differently than the base IFD — the one being read is checked.
    fn aim(&mut self, image: usize) -> Result<Pixel, String> {
        if let Some((at, pixel)) = self.at
            && at == image
        {
            return Ok(pixel);
        }
        self.decoder.seek_to_image(image).map_err(|e| format!("tiff: {}", e))?;
        chunk_grid(&mut self.decoder)?;
        let pixel = pixel(self.decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
        self.at = Some((image, pixel));
        Ok(pixel)
    }
}

impl<R: Read + Seek> Chunked for Chunks<R> {
    fn chunk(&mut self, image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String> {
        let pixel = self.aim(image)?;
        let (dw, dh) = self.decoder.chunk_data_dimensions(index);
        let data = self.decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        Ok((chunk_rgba(&self.mapping, &data, pixel, dw, dh)?, dw, dh))
    }

    /// Where the chunks lie in the file is told by the image tags — offsets and
    /// lengths of tiles or strips; the decoder aimed at the image has already
    /// read them.
    fn prefetch(&mut self, image: usize, indices: &[u32]) -> Result<(), String> {
        self.aim(image)?;
        let (offsets, counts) = match self.decoder.get_tag_unsigned::<u32>(Tag::TileWidth).is_ok() {
            true => (Tag::TileOffsets, Tag::TileByteCounts),
            false => (Tag::StripOffsets, Tag::StripByteCounts),
        };
        let offsets = self.decoder.get_tag_u64_vec(offsets).map_err(|e| format!("tiff: {}", e))?;
        let counts = self.decoder.get_tag_u64_vec(counts).map_err(|e| format!("tiff: {}", e))?;
        let ranges: Vec<(u64, u64)> = indices
            .iter()
            .filter_map(|&index| Some((*offsets.get(index as usize)?, *counts.get(index as usize)?)))
            .collect();
        veldsdk::abi::resource_prefetch(self.resource, &ranges).map_err(|e| e.to_string())
    }
}

/// Pointwise reading of a level's tiles — the driver over the file's chunks.
pub fn produce_direct<R: Read + Seek>(
    reader: R,
    resource: u64,
    info: &Info,
    layout: &Layout,
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::new(reader, resource, layout)?;
    grid::direct(&mut chunks, &layout.grid, (info.width, info.height), level, wants, emit)
}

/// A sequential pass over the base IFD — the driver over the file's chunks.
pub fn produce_pass<R: Read + Seek>(reader: R, resource: u64, info: &Info, layout: &Layout, emit: Emit) -> Result<(), String> {
    let mut chunks = Chunks::new(reader, resource, layout)?;
    grid::pass(&mut chunks, &layout.grid, (info.width, info.height), emit)
}

// ── Common ──────────────────────────────────────────────────────

/// Planar layout (a plane per channel) chunks differently: a chunk carries one
/// plane, while the assembly here counts it as all channels interleaved. A
/// refusal, not a mash of grey planes.
/// The chunk size of the image the decoder stands on — together with the
/// checks without which it cannot be read.
///
/// This description is asked by chunk reading ([`Chunks::aim`]) and the stretch
/// sampling. They need one and the same thing — the layout must be interleaved,
/// the chunk size non-zero, and the chunk itself fit in memory, because it is
/// decoded whole. The size check stands before the reading and measures exactly
/// the chunk: `grid::REGION_CAP` measures the tile region, not how its pieces
/// lie.
fn chunk_grid<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(u32, u32), String> {
    ensure_chunky(decoder)?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: zero chunk size".to_string());
    }
    // A fresh chunk — raw samples and RGBA at once; `Grid::chunk_bytes` counts
    // the same sum.
    let depth = depth_of(decoder)?;
    Peak::new()
        .with("fresh chunk", u64::from(cw) * u64::from(ch) * (u64::from(depth) + 4))
        .admit()
        .map_err(|why| format!("tiff: chunk {}×{}: {}", cw, ch, why))?;
    Ok((cw, ch))
}

fn ensure_chunky<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    let planar = decoder.get_tag_unsigned::<u16>(Tag::PlanarConfiguration).unwrap_or(1);
    if planar != 1 {
        return Err("tiff: planar sample layout is not supported".to_string());
    }
    Ok(())
}

/// A chunk into RGBA — with a check that it came out whole.
///
/// The stretch silently shortens the output when fewer samples arrive than the
/// chunk size promises, while both the tile and the strip assembly blit by the
/// promised size. A truncated chunk of a broken file must end with an error,
/// not with an out-of-bounds slice: the latter for a wasm module is a trap,
/// after which the host raises the instance anew, and the caller is left
/// waiting for an answer no one will ever send.
fn chunk_rgba(
    mapping: &Mapping,
    data: &DecodingResult,
    pixel: Pixel,
    dw: u32,
    dh: u32,
) -> Result<Vec<u8>, String> {
    let pixels = (dw as usize) * (dh as usize);
    let rgba = mapping.rgba(&typed(data)?, pixel, pixels);
    match rgba.len() == pixels * 4 {
        true => Ok(rgba),
        false => Err(format!(
            "tiff: chunk {}×{} arrived incomplete — {} pixels instead of {}",
            dw,
            dh,
            rgba.len() / 4,
            pixels
        )),
    }
}

/// The raster's color model in numbers: how many samples per pixel and how many
/// bits per sample. One parse for everyone who asks: otherwise the list of
/// accepted models would live in two shapes and diverge silently.
fn model(color: tiff::ColorType) -> Result<(Pixel, u8), String> {
    match color {
        tiff::ColorType::Gray(bits) => Ok((Pixel::named(1), bits)),
        tiff::ColorType::GrayA(bits) => Ok((Pixel::named(2), bits)),
        tiff::ColorType::RGB(bits) => Ok((Pixel::named(3), bits)),
        tiff::ColorType::RGBA(bits) => Ok((Pixel::named(4), bits)),
        // That is what the crate calls anything with more than one sample and
        // no color interpretation — the usual GDAL output for any band stack.
        // This is not an "unsupported model" but a stack of measured values:
        // the first one is shown, the rest are stepped past (see `Pixel::stack`).
        tiff::ColorType::Multiband { bit_depth, num_samples } => {
            Ok((Pixel::stack(usize::from(num_samples)), bit_depth))
        }
        other => Err(format!("tiff: color model {:?} is not supported", other)),
    }
}

fn pixel(color: tiff::ColorType) -> Result<Pixel, String> {
    Ok(model(color)?.0)
}

/// Bytes per pixel in the raw samples of the image the decoder stands on:
/// channels × bytes per sample. A sub-byte depth is rejected by
/// [`ensure_readable`].
fn depth_of<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<u32, String> {
    let (pixel, bits) = model(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
    Ok(pixel.channels as u32 * u32::from(bits.div_ceil(8)))
}

/// Refusal on what would otherwise fall in the middle of a pass.
///
/// This is asked at describe time, not at produce time, because the color model
/// and depth are the same for all tiles of a file: a refusal here is an honest
/// "nothing to show", while the same refusal on the first tile would look like
/// "the frame is incomplete" and invite a retry.
///
/// A sub-byte depth is named separately: samples of such depth the crate does
/// not unpack but hands over as they lie — several to a byte — while the tile
/// assembly assumes a whole number of them per pixel. Without the separate name
/// this is not a refusal but a buffer overrun — a trap for the whole instance.
fn ensure_readable<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    let color = decoder.colortype().map_err(|e| format!("tiff: {}", e))?;
    let (_, bits) = model(color)?;
    if bits < 8 {
        return Err(format!(
            "tiff: {} bits per sample — such depth does not unpack into pixels",
            bits
        ));
    }
    ensure_decodable(decoder)?;
    ensure_sampled(decoder, bits)
}

/// Compressions that our assembled decoder set parses.
///
/// The list is derived from the `tiff` crate features in `config.yaml`:
/// `deflate`, `fax`, `jpeg`, `lzw` from the defaults plus our `zstd`. Fax here
/// is group 4 only — the crate does not decode group 3 at all — and `webp` we
/// do not build.
///
/// Diverging from `config.yaml`, the list would either kill the readable or let
/// the unreadable through until the first tile — exactly the failure this list
/// exists to prevent. The compiler does not catch such a divergence, so a
/// buildgen test keeps them in agreement.
const DECODED: [(u16, &str); 8] = [
    (1, "no compression"),
    (4, "Fax4"),
    (5, "LZW"),
    (7, "JPEG"),
    (8, "Deflate"),
    (0x8005, "PackBits"),
    (0x80B2, "Deflate (old code)"),
    (0xC350, "ZSTD"),
];

/// The refusal on compression — at describe time, not at the first chunk.
///
/// The whole difference is in how much a person manages to do in vain: a file
/// has one compression for all its tiles, and it can be learned from the
/// header. A refusal postponed until reading costs gigabytes of downloads — and
/// arrives after the whole path to it looked sound.
fn ensure_decodable<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    // No tag — per the spec that is "no compression".
    let code = decoder.get_tag_unsigned::<u16>(Tag::Compression).unwrap_or(1);
    if decodable(code) {
        return Ok(());
    }
    let said = match code {
        2 => "Huffman",
        3 => "Fax3",
        6 => "old JPEG",
        0xC351 => "WebP",
        34_887 => "LERC",
        34_925 => "LZMA",
        50_002 => "JPEG XL",
        _ => "unknown",
    };
    let known = DECODED.iter().map(|(_, name)| *name).collect::<Vec<_>>().join(", ");
    Err(format!("tiff: compression {} ({}) is not parsed — readable: {}", said, code, known))
}

/// Whether compression with this code is parsed.
///
/// Separated from tag reading because the rule is pure and checkable without a
/// file.
fn decodable(code: u16) -> bool {
    DECODED.iter().any(|(known, _)| *known == code)
}

/// Whether a sample of this format and depth unfolds into brightness.
///
/// The pair to [`typed`], and they must agree mechanically: here the refusal
/// comes at describe time, there — at the chunk, and diverging they give either
/// a refusal over the readable or gigabytes downloaded in vain. Sealed by a
/// test.
fn sampled(format: u16, bits: u8) -> bool {
    matches!((format, bits), (1, 8) | (1, 16) | (2, 16) | (3, 32))
}

/// The sample format — the same and for the same reason (see
/// [`ensure_decodable`]).
///
/// The display unfolds four combinations of format and depth (see [`typed`]),
/// and which one the file has is stated in the header. The dearest here is the
/// complex sample: that is how radar before aperture synthesis (Sentinel-1 SLC)
/// is written, and that is gigabytes downloaded for a refusal on the very first
/// chunk.
fn ensure_sampled<R: Read + Seek>(decoder: &mut Decoder<R>, bits: u8) -> Result<(), String> {
    // The tag holds one sample per band; the first is taken — a file is complex
    // or integer as a whole, not per band. No tag — "unsigned".
    let format = decoder
        .get_tag_u16_vec(Tag::SampleFormat)
        .ok()
        .and_then(|formats| formats.first().copied())
        // The crate does not parse every list — one written as LONG arrives
        // here as a scalar, and it must be asked a second way rather than
        // silently taken as unsigned.
        .or_else(|| decoder.get_tag_unsigned::<u16>(Tag::SampleFormat).ok())
        // No tag at all — per the spec that is "unsigned integer".
        .unwrap_or(1);
    if sampled(format, bits) {
        return Ok(());
    }
    let said = match format {
        1 => "unsigned",
        2 => "signed",
        3 => "floating",
        5 => "complex integer",
        6 => "complex floating",
        _ => "unknown kind",
    };
    Err(format!(
        "tiff: sample {} of {} bits does not unfold into brightness — readable: \
         integers of 8 and 16 bits, signed of 16 and floating of 32",
        said, bits
    ))
}

/// The chunk's samples in typed form — what the display can break down.
/// Other depths are a refusal: no file with them occurs in the catalog, and
/// "just in case" support is checked by nothing.
fn typed(data: &DecodingResult) -> Result<Samples<'_>, String> {
    use tiff::decoder::DecodingResult as R;
    Ok(match data {
        R::U8(v) => Samples::U8(v),
        R::U16(v) => Samples::U16(v),
        R::I16(v) => Samples::I16(v),
        R::F32(v) => Samples::F32(v),
        other => {
            return Err(format!(
                "tiff: sample depth is not supported ({})",
                match other {
                    R::U32(_) => "u32",
                    R::U64(_) => "u64",
                    R::F16(_) => "f16",
                    R::F64(_) => "f64",
                    _ => "signed",
                }
            ))
        }
    })
}

/// The file's stretch — the ready one if it was already counted, otherwise
/// computed and remembered (see [`Layout::stretch`]).
///
/// Where the decoder stays depends on whether it was counted: on the ready one
/// it does not move, on the computed one it stands on [`Layout::stats`]. One
/// must not rely on this — the caller aims it itself.
fn stretch<R: Read + Seek>(layout: &Layout, decoder: &mut Decoder<R>) -> Result<Mapping, String> {
    if let Some(ready) = *layout.stretch.borrow() {
        return Ok(ready);
    }
    let built = mapping(decoder, layout.stats())?;
    *layout.stretch.borrow_mut() = Some(built);
    Ok(built)
}

/// The file's display mapping: identity for bytes, a percentile stretch for
/// wide formats (see radiometry.rs). The sample is up to four chunks scattered
/// from the `stats` IFD, thinned to [`radiometry::STRETCH_SAMPLES`]. The choice
/// is deterministic: one file — one stretch, whichever tiles you ask and in
/// whichever order.
///
/// After the return the decoder stands on the `stats` IFD.
fn mapping<R: Read + Seek>(decoder: &mut Decoder<R>, stats: usize) -> Result<Mapping, String> {
    // GDAL_NODATA is written to the base IFD — read before re-aiming.
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim().trim_end_matches('\0').parse::<f32>().ok());
    let (_, bits) = model(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
    if bits <= 8 {
        return Ok(Mapping::identity(nodata));
    }

    decoder.seek_to_image(stats).map_err(|e| format!("tiff: {}", e))?;
    let (w, h) = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;
    let (cw, ch) = chunk_grid(decoder)?;
    let pixel = pixel(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;

    let total = w.div_ceil(cw) * h.div_ceil(ch);
    let mut picks = [0, total / 3, 2 * total / 3, total.saturating_sub(1)].to_vec();
    picks.dedup();

    let mut values = Vec::new();
    for &index in &picks {
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let samples = typed(&data)?;
        // The sampling goes over pixels and takes only the color samples from
        // each: exactly those go into the frame (see `Mapping::rgba`), while
        // alpha and unshown bands would shift the percentiles without affecting
        // anything. The difference is not subtle: for grey with alpha, a
        // constant alpha of 65535 is half the sample at the top edge of the
        // format, and the image comes out almost black; in a GDAL band stack a
        // 0..1 mask next to amplitude drags the lower edge to zero.
        let pixels = samples.len() / pixel.channels.max(1);
        let step = (pixels * picks.len() / radiometry::STRETCH_SAMPLES).max(1);
        for at in (0..pixels).step_by(step) {
            for channel in 0..pixel.colors() {
                let v = samples.get(at * pixel.channels + channel);
                if radiometry::is_data(v, nodata) {
                    values.push(v);
                }
            }
        }
    }
    match percentile_stretch(&mut values) {
        Some((lo, hi)) => Ok(Mapping::stretched(lo, hi, nodata)),
        // The whole sample is "no data": there is nothing to stretch, and a
        // stretch must not be invented. Any bound assigned here whitens
        // everything above it: for a [0, 1] stretch the unit already comes out
        // white. The values are therefore taken as bytes — the only thing we do
        // not invent — and for a byte raster this is exactly right, while a wide
        // frame is almost all transparent anyway: a sample of a million counts
        // found not a single valid one in it (see `Mapping::identity`).
        None => Ok(Mapping::identity(nodata)),
    }
}
