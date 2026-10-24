//! Format adapters: describe a raster by its header and produce tiles.
//!
//! The format is determined by content — an extension lies more often than a
//! header — and bytes arrive in windows from the resource: whether it is a file,
//! the network or memory is not visible from here. The formats differ in one
//! thing only: whether one can yield an arbitrary tile cheaply. The level table
//! (`table.rs`) answers this: a pointwise level reads exactly the needed chunks
//! (`grid.rs`), the rest go through a pass that builds all levels at once in a
//! cascade (see cascade.rs), and the requested tiles leave as the pass goes,
//! top down.

use std::cell::Cell;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::rc::Rc;

use image::ImageFormat;

use super::cascade::Emit;
use super::pyramid;

pub mod codec;
pub mod excerpt;
pub mod full;
pub mod grid;
pub mod jp2;
pub mod jpeg;
pub mod netcdf;
pub mod png;
pub mod radiometry;
pub mod table;
pub mod tiff;

/// Ceiling on the source side. It guards two different things at once.
///
/// **Plausibility.** A raster with a six-digit side is more often not a raster
/// but a header parsed the wrong way, and learning this is cheaper before reading.
///
/// **The price of the cascade — for those who do not reckon it.** The cascade
/// strips grow with the width (about five kilobytes per column,
/// [`crate::cascade::bytes`]): at this side they ask for about a third of the
/// instance's free memory, that is, they pass. The chunk grid adds this price to
/// the strip and the fresh chunk itself (`Grid::pass_peak`) and therefore rejects
/// earlier; PNG and JPEG have no count of their own, and by side this is their
/// only bound.
pub const MAX_SOURCE_SIDE: u32 = 65_536;

/// Ceiling for paths without streaming unfolding (gif/bmp/webp, Adam7-PNG,
/// a decoded JPEG frame): such a frame lives in memory whole.
pub const FULL_DECODE_BUDGET: u64 = 256 * 1024 * 1024;

/// Whether a `width`×`height` frame fits into RGBA under the frame ceiling
/// entirely. One check for both the level table and the paths themselves: the
/// "fits" column and the refusal before allocating the frame must agree.
pub fn frame_fits(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) * 4 <= FULL_DECODE_BUDGET
}

/// What is known about the raster without decoding.
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
    /// Georeference grid, if the file carries one: where each pixel lies on the
    /// Earth. Empty for formats with no room for it — this is not "unknown
    /// where from" but "the file does not say".
    pub ties: Vec<Tie>,
    /// Projection binding — for a raster that does not lie in degrees. Mutually
    /// exclusive with [`Info::ties`]: the nodes are declared in degrees, and the
    /// zone's meters have no place in them (see [`Placement`]).
    pub placement: Option<Placement>,
    /// Instrument frame in which the raster itself is recorded. It is needed
    /// not by the raster but by the coordinates from the neighboring file: the
    /// grid there stands in its own frame, and it can meet the raster only
    /// through this pair (see [`netcdf::geolocation`]). Filled by one NetCDF —
    /// the others say nothing of the kind.
    pub frame: Option<netcdf::Frame>,
    /// The file started to speak about the binding, but it could not be taken —
    /// the reason in words.
    ///
    /// Empty means one of two things, and there is no need to tell them apart:
    /// either the binding was taken, or the file never raised the subject. The
    /// caveat is needed exactly where what the file said did not arrive: by
    /// empty [`Info::ties`] "the file does not say" cannot be told from "it was
    /// said, but did not read", and in the second case the snapshot would lie as
    /// a guess and look real.
    ///
    /// The adapter speaks, not the consumer: only the one who read the file
    /// knows why it did not work out.
    pub binding_trouble: Option<String>,
    /// Which of the file's quantities is shown — for a file of many quantities
    /// (NetCDF): the adapter's choice or the one named by the caller (`wanted`
    /// at [`describe`]). Empty for a snapshot: it has one quantity, and it has
    /// no name.
    pub variable: Option<Variable>,
    /// All quantities of the file that could be shown, in the adapter's order of
    /// preference; the shown one is among them. Fitness here is judged by the
    /// headers, not the samples: a quantity empty or flat in this granule is in
    /// the list. Empty for a snapshot.
    pub variables: Vec<Variable>,
}

/// A quantity of a file of many quantities — shown or one of those listed: how
/// it is named in the file and what the file called it in words. Exactly what
/// the viewer needs to understand what they are looking at, and nothing for the
/// display — the stretch and the layout live in [`Kind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// Path inside the file (`/PRODUCT/carbonmonoxide_total_column`).
    pub path: String,
    /// `long_name` or `standard_name`; empty — the file did not name it.
    pub said: String,
    /// `units`, as written in the file; empty — not written.
    pub units: String,
}

/// A node of the georeference grid: a raster pixel and its place on the Earth,
/// WGS84.
///
/// A grid, not a frame: a radar image lies in acquisition geometry, and a
/// rectangle in degrees does not describe it — the GeoTIFF of such a raster
/// carries a lattice of control points (21×21 for a Sentinel-1 granule), while
/// a frame would only exist for a projection with north up.
pub struct Tie {
    pub px: f64,
    pub py: f64,
    pub lat: f64,
    pub lon: f64,
}

/// Whether the pair recorded in a node is valid.
///
/// One rule for all providers on purpose. An unfilled NetCDF sample is marked
/// not only by `NaN` but also by a number — −999 for SYNERGY, 9.96921e36 for
/// CF — and it shows on the globe; the globe needs no separate finiteness
/// check, comparisons with `NaN` are false, and infinity falls off the edge.
/// GeoTIFF tags have no filler, but the number there is read from a file, and
/// the file comes from the network: an infinite longitude, once it reaches the
/// consumer, drives the rotation into arithmetic with no place to put it.
///
/// The longitude is checked on par with the latitude: a lattice checked only by
/// latitude would let a hole through by longitude. The circle for the longitude
/// is full with margin — files write it both as −180…180 and as 0…360, and the
/// consumer unwinds it (`Grid::unwind`).
pub fn placed(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat) && (-360.0..=360.0).contains(&lon)
}

/// Binding of a raster lying in a projection: the EPSG code of the coordinate
/// system and the linear transform of a pixel into its meters.
///
/// A code, not a zone: the code is written in the file, while "zone 38 north"
/// is already an interpretation, and the one who interprets it is the one who
/// knows the Earth. Translating here would mean keeping a second copy of the
/// projection math.
///
/// A six, not a "origin and step" pair: it covers both GeoTIFF bindings — the
/// point with the pixel step and the matrix — that is, a rotated raster too.
pub struct Placement {
    pub epsg: u32,
    /// `x = a[0]·i + a[1]·j + a[2]`, `y = a[3]·i + a[4]·j + a[5]`, where (i, j)
    /// is the pixel corner, zero at the top left of the raster, the j axis
    /// pointing down. The half pixel of RasterPixelIsPoint is removed here too,
    /// just as it is removed at the nodes.
    pub affine: [f64; 6],
}

pub enum Kind {
    /// PNG streamed by rows; interlaced (Adam7) — as a whole frame.
    Png { interlaced: bool },
    Jpeg,
    /// JPEG 2000: codestream tiles are the driver's chunks, copies are the
    /// resolution levels.
    Jp2(jp2::Layout),
    Tiff(tiff::Layout),
    /// Formats without a streaming path: decoded whole, they are small by nature.
    Full(ImageFormat),
    /// NetCDF-4: not a picture but a set of measured quantities. One of them is
    /// chosen as shown at description, and its grid — full-width row windows —
    /// are the driver's chunks (see `netcdf::Layout`).
    Netcdf(netcdf::Layout),
}

impl Info {
    /// A raster without georeference: the file has no room for control points
    /// at all. This is how all formats are described except the two that speak
    /// of their place on the Earth themselves — GeoTIFF and NetCDF.
    pub fn plain(width: u32, height: u32, kind: Kind) -> Self {
        Self {
            width,
            height,
            kind,
            ties: Vec::new(),
            placement: None,
            frame: None,
            binding_trouble: None,
            variable: None,
            variables: Vec::new(),
        }
    }
}

/// Raster header: format, dimensions, layout. Cheap even for a gigabyte file —
/// headers and catalogs are read, not pixels.
pub fn describe(resource_id: u64, len: u64, wanted: &str, bytes: &Rc<Cell<u64>>) -> Result<Info, String> {
    let mut reader = Metered::new(resource_id, len, bytes.clone());
    let mut head = [0u8; 32];
    let read = reader.read(&mut head).map_err(|e| format!("reading the header: {}", e))?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    // A quantity is named for a file of many quantities; a snapshot has one and
    // it has no name, so one named there is the caller's mistake, not a choice.
    if !wanted.is_empty() && !head.starts_with(netcdf::MAGIC) {
        return Err(format!("quantity '{}' requested of a single-quantity file", wanted));
    }

    // The `image` crate knows neither JPEG 2000, nor NetCDF, nor BigTIFF — their
    // signatures are checked before it.
    if head.starts_with(jp2::JP2_MAGIC) || head.starts_with(jp2::CODESTREAM_MAGIC) {
        let info = jp2::describe(reader, len)?;
        return checked(info);
    }
    if head.starts_with(netcdf::MAGIC) {
        // Through the resource, not the reader: HDF5 walks the file at random
        // with absolute offsets, and wrapping this in a sequential stream would
        // take away exactly what makes it cheap (see `netcdf::Resource`).
        drop(reader);
        let info = netcdf::describe(resource_id, len, wanted)?;
        return checked(info);
    }
    // Classic NetCDF-3 is a different format, and there is no reader for it
    // here. It is named separately because the general refusal below lists
    // NetCDF among the openable ones: said over a `.nc` file, it would read as
    // a breakage.
    if netcdf::CLASSIC.iter().any(|magic| head.starts_with(magic)) {
        return Err("this is classic NetCDF-3, while what is read is NetCDF-4 — the one written as HDF5"
            .to_string());
    }
    if tiff::BIG_MAGIC.iter().any(|magic| head.starts_with(magic)) {
        let info = tiff::describe(reader)?;
        return checked(info);
    }

    // The refusal lists what is openable at all: a file without an image
    // signature is a common case in a catalog (raw L0, annotations, XML), and
    // "unrecognized" without a list reads as a breakage, not as an answer.
    let format = image::guess_format(&head[..read]).map_err(|_| {
        "by the header this is not an image \
         (opened are PNG, JPEG, TIFF, JPEG 2000, NetCDF, GIF, BMP, WebP)"
            .to_string()
    })?;

    let info = match format {
        ImageFormat::Png => png::describe(reader),
        ImageFormat::Jpeg => jpeg::describe(reader),
        ImageFormat::Tiff => tiff::describe(reader),
        // The set must match the features of the `image` crate in config.yaml —
        // a feature without a match arm silently yields "not supported".
        ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP => full::describe(reader, format),
        other => Err(format!("format {:?} is not supported", other)),
    }?;
    checked(info)
}

/// Common limits of a described raster — whichever adapter described it.
fn checked(info: Info) -> Result<Info, String> {
    if info.width == 0 || info.height == 0 {
        return Err("empty raster".to_string());
    }
    if info.width > MAX_SOURCE_SIDE || info.height > MAX_SOURCE_SIDE {
        return Err(format!(
            "{}×{}: the side exceeds the ceiling {} — a raster never has that much, \
             and the pass strips over it would not fit into the instance memory",
            info.width, info.height, MAX_SOURCE_SIDE
        ));
    }
    // A source where no level fits into memory is not described: the
    // description would promise a detail limit at the top, while the tiles
    // would never arrive. The refusal names the terms of the cheapest row.
    let rows = info.levels();
    if let Some(top) = rows.last()
        && rows.iter().all(|row| !row.fits)
    {
        return Err(format!("no level fits into memory: {}", top.peak.note()));
    }
    Ok(info)
}

/// Produce the tiles of `level`. `wants` is used by the pointwise path only —
/// it reads exactly what is requested; the pass hands `emit` all tiles of all
/// levels, and the receiver decides who needs which.
///
/// The arm is chosen by the row of the level table ([`Info::level`]): the same
/// row that went to the consumer in the description (`Described.levels`) — the
/// price promised to them and the price paid here are counted by one row.
pub fn produce(
    resource_id: u64,
    len: u64,
    info: &Info,
    level: u32,
    wants: &[(u32, u32)],
    bytes: &Rc<Cell<u64>>,
    emit: Emit,
) -> Result<(), String> {
    let reader = || Metered::new(resource_id, len, bytes.clone());
    let row = info.level(level).ok_or_else(|| {
        format!("there is no level {}: the raster has {}", level, pyramid::level_count(info.width, info.height))
    })?;
    match (&info.kind, row.serve) {
        // Pointwise — where the tile window is a real window. This is decided by
        // the level, not the source: a striped granule is read pointwise nearby
        // and by a pass from afar, and the coarse edge is covered by the pass. A
        // level taken by a pass arrives slowly; a level taken by a refusal never
        // arrives at all — so the table calls pointwise only what the driver
        // will yield.
        (Kind::Tiff(layout), table::Serve::Pointwise) => {
            tiff::produce_direct(reader(), resource_id, info, layout, level, wants, emit)
        }
        (Kind::Tiff(layout), table::Serve::Pass { .. }) => {
            tiff::produce_pass(reader(), resource_id, info, layout, emit)
        }
        // The codec needs its own stream per factor, and JP2 spawns its own
        // readers.
        (Kind::Jp2(layout), table::Serve::Pointwise) => {
            jp2::produce_direct(resource_id, bytes, info, layout, level, wants, emit)
        }
        (Kind::Jp2(layout), table::Serve::Pass { .. }) => jp2::produce_pass(resource_id, bytes, info, layout, emit),
        // HDF5 walks the file at random with absolute offsets, and NetCDF
        // spawns its own resource reader (see `netcdf::Resource`).
        (Kind::Netcdf(layout), table::Serve::Pointwise) => {
            netcdf::produce_direct(resource_id, len, bytes, info, layout, level, wants, emit)
        }
        (Kind::Netcdf(layout), table::Serve::Pass { .. }) => {
            netcdf::produce_pass(resource_id, len, bytes, info, layout, emit)
        }
        // The rest have a single path, and the table row only says where it
        // starts.
        (Kind::Png { .. }, _) => png::produce_pass(reader(), info, emit),
        (Kind::Jpeg, _) => jpeg::produce(reader(), info, level, emit),
        (Kind::Full(format), _) => full::produce(reader(), info, *format, emit),
    }
}

/// Unfolding of samples into RGBA8 — shared by all adapters: 1 channel — gray,
/// 2 — gray with alpha, 3 — RGB, 4 — as is.
/// How many steps the color spreads under the transparent. Filtering mixes a
/// texel with its immediate neighbors, so spreading past the second ring is
/// pointless.
const BLEED_STEPS: u32 = 2;

/// Color under the transparent — from the nearest opaque neighbor.
///
/// Under a fully transparent pixel lies black: it has no color, and zero keeps
/// the output deterministic (see `resample`). On screen this zero is visible.
/// Both the canvas and the globe filter the texture linearly and blend with
/// non-premultiplied alpha, so at the edge of a `nodata` field the interpolation
/// gives half the color at half the opacity — a dark halo a texel wide. A cell
/// drawn by an ancestor stretches it twofold at every step, so at a coarse step
/// this is a noticeable dark outline around the image.
///
/// Alpha is left untouched meanwhile: the edge stays exactly where it was, and
/// color starts mixing with color instead of color with blackness.
pub fn bleed_alpha(rgba: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return;
    }
    let mut coloured: Vec<bool> = (0..w * h).map(|at| rgba[at * 4 + 3] != 0).collect();
    // A fully opaque tile has no edge; a fully transparent one has nowhere to
    // take color from.
    if coloured.iter().all(|has| *has) || coloured.iter().all(|has| !has) {
        return;
    }

    for _ in 0..BLEED_STEPS {
        // A snapshot of the previous ring: without it the color would travel
        // into the field in one pass, and the spreading would depend on the
        // traversal order.
        let known = coloured.clone();
        let mut spread = false;
        for at in 0..w * h {
            if known[at] {
                continue;
            }
            let (x, y) = (at % w, at / w);
            let neighbours = [
                (x > 0).then(|| at - 1),
                (x + 1 < w).then_some(at + 1),
                (y > 0).then(|| at - w),
                (y + 1 < h).then_some(at + w),
            ];
            let Some(from) = neighbours.into_iter().flatten().find(|near| known[*near]) else {
                continue;
            };
            let colour = [rgba[from * 4], rgba[from * 4 + 1], rgba[from * 4 + 2]];
            rgba[at * 4..at * 4 + 3].copy_from_slice(&colour);
            coloured[at] = true;
            spread = true;
        }
        if !spread {
            break;
        }
    }
}

pub fn to_rgba(samples: &[u8], pixel: radiometry::Pixel, pixels: usize) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in samples.chunks_exact(pixel.channels).take(pixels) {
        let (rgb, alpha) = match (pixel.colors(), pixel.has_alpha()) {
            (1, false) => ([px[0], px[0], px[0]], 255),
            (1, true) => ([px[0], px[0], px[0]], px[pixel.channels - 1]),
            (_, false) => ([px[0], px[1], px[2]], 255),
            (_, true) => ([px[0], px[1], px[2]], px[pixel.channels - 1]),
        };
        rgba.extend_from_slice(&rgb);
        rgba.push(alpha);
    }
    rgba
}

/// A resource reader with a counter of what has been read — an honest progress
/// of a long pass grows out of it. The counter is the farthest reached position,
/// not the sum of reads: repeated windows after a seek back do not approach the
/// end of the file and are not progress, while the sum would overrun the size.
/// Shared with the tile receiver through Rc — the instance is single-threaded.
pub struct Metered {
    inner: veldsdk::ResourceReader,
    /// Current position: both read and consume move it themselves, because
    /// asking the resource for it would be an extra ABI call per read.
    pos: u64,
    consumed: Rc<Cell<u64>>,
}

impl Metered {
    pub fn new(resource_id: u64, len: u64, consumed: Rc<Cell<u64>>) -> Self {
        Self { inner: veldsdk::ResourceReader::new(resource_id, len), pos: 0, consumed }
    }

    fn advance(&mut self, n: usize) {
        self.pos += n as u64;
        if self.pos > self.consumed.get() {
            self.consumed.set(self.pos);
        }
    }
}

impl Read for Metered {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.advance(n);
        Ok(n)
    }
}

impl BufRead for Metered {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.advance(amt);
        self.inner.consume(amt);
    }
}

impl Seek for Metered {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let at = self.inner.seek(pos)?;
        self.pos = at;
        Ok(at)
    }
}
