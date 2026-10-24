//! JPEG 2000 (JP2/J2C): quicklooks and Sentinel-2 granules — in driver chunks.
//! A chunk is a codestream tile at a resolution factor: openjp2 decodes it
//! behind the unsafe island (`codec.rs`), while which chunks to read and when
//! to make a pass is decided by the shared chunk grid driver (`grid.rs`) from
//! the layout in the main header ([`Layout`]). Description never runs the
//! codec: dimensions, the tile grid and the resolution count are read by our
//! own parse of the file head ([`codestream`]), the georeference — by the
//! GMLJP2 box ([`gml_placement`]). A tile is fed to the codec as an excerpt
//! via the TLM index (`excerpt.rs`), and without TLM — as the whole file,
//! which the codec walks by itself. What is decided and why —
//! `docs/decisions/0002-jpeg2000-decoder.md` and `0004-jp2-excerpt.md`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::rc::Rc;

use super::super::cascade::Emit;
use super::super::pyramid;
use super::codec::{Decoder, Format, Header, Tile};
use super::excerpt;
use super::grid::{self, Chunked, Grid, Overview};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples};
use super::{Info, Kind, Metered, Placement};

/// How much of the file head is enough for the header: signature boxes, ftyp,
/// jp2h come first, and in a raw codestream SIZ — right after SOC.
const HEAD: usize = 64 * 1024;

/// Bytes per sample inside the decoder: `i32` planes for the image and the same
/// again for the tile coder inside — chunk memory is counted by them, not by
/// the file bit depth.
const SAMPLE_BYTES: u32 = 8;

/// File layout for the driver: the codestream tile grid with copies by
/// resolution — plus what the driver need not know but a chunk does.
pub struct Layout {
    pub grid: Grid,
    /// Codestream tiles across and down — one grid for all factors: the codec's
    /// tile index does not depend on resolution.
    pub tiles: (u32, u32),
    /// File length: the codec uses it to compute the stream remainder.
    pub len: u64,
    format: Format,
    /// Sample bit depth: up to a byte no stretch is needed, and this is decided
    /// without the codec.
    precision: u8,
    /// Zero is "no data": a Sentinel-2 granule has valid values 1..255, and
    /// scene gaps arrive as zero. Set only for a file with GMLJP2 georeference:
    /// in an arbitrary JP2 nothing forbids black from being a color.
    nodata: Option<f32>,
    /// Display stretch for samples wider than a byte — one per file, otherwise
    /// neighboring tiles would diverge in brightness (see `tiff::Layout`).
    stretch: RefCell<Option<Mapping>>,
    /// Tile-part index by TLM; without it the codec finds the tile itself,
    /// walking SOT.
    index: Option<excerpt::Index>,
}

impl Layout {
    /// A copy at factor `f` exists while the tile divides by `2^f` without
    /// remainder: this keeps the tile grid uniform on the reduced lattice, and
    /// it is also the chunk grid of the copy. Coarser levels take the finest
    /// copy by the driver's general rule.
    pub fn of(
        codestream: &Codestream,
        len: u64,
        format: Format,
        nodata: Option<f32>,
        index: Option<excerpt::Index>,
    ) -> Self {
        let width = codestream.canvas.0 - codestream.origin.0;
        let height = codestream.canvas.1 - codestream.origin.1;
        let (tw, th) = codestream.tile;
        let overviews = (1..u32::from(codestream.resolutions))
            .take_while(|factor| tw % (1 << factor) == 0 && th % (1 << factor) == 0)
            .map(|factor| Overview {
                image: factor as usize,
                width: pyramid::level_size(width, factor),
                height: pyramid::level_size(height, factor),
                chunk: (tw >> factor, th >> factor),
            })
            .collect();
        let depth = u32::from(codestream.components) * SAMPLE_BYTES;
        Self {
            grid: Grid { tiled: true, chunk: codestream.tile, overviews, depth },
            tiles: codestream.grid(),
            len,
            format,
            precision: codestream.precision,
            nodata,
            stretch: RefCell::new(None),
            index,
        }
    }

    /// Whether the tile is read as a TLM excerpt rather than by walking SOT.
    pub fn indexed(&self) -> bool {
        self.index.is_some()
    }
}

pub fn describe(mut reader: Metered, len: u64) -> Result<Info, String> {
    // The length is taken as `u64` and compared in the same width: the
    // module's `usize` is thirty-two-bit, and narrowing it first would make a
    // four-gigabyte file with a hundred bytes extra give a head of a hundred
    // bytes — and that is not a read failure, but a calm "no signature"
    // answer about a perfectly good raster.
    let mut head = vec![0u8; len.min(HEAD as u64) as usize];
    reader.read_exact(&mut head).map_err(|e| format!("jp2: header read: {}", e))?;
    let (width, height) = header_dims(&head)?;
    let layout = codestream(&head)?;
    // A non-zero canvas origin shifts the decoder's reduced lattice by a pixel
    // against the pyramid ladder (ADR 0002): such a file is refused outright,
    // not read with seams.
    if layout.origin != (0, 0) || layout.tile_origin != (0, 0) {
        return Err(format!(
            "jp2: canvas origin {:?} and tile grid origin {:?} are not at zero — the \
             decoder's reduced lattice would diverge from the pyramid ladder",
            layout.origin, layout.tile_origin
        ));
    }
    if (layout.canvas.0 - layout.origin.0, layout.canvas.1 - layout.origin.1) != (width, height) {
        return Err("jp2: container and codestream dimensions diverge".to_string());
    }
    let (across, down) = layout.grid();
    // The index is built here too, from the same head: a tile is read as an
    // excerpt only if TLM is present and matches the file, and the description
    // line says so — the granule is measured by it.
    let index = excerpt::Index::of(&head, len);
    veldsdk::log::debug!(target: "perf",
        "jp2 {}×{}: tiles {}×{} of {}×{}, components {}, resolutions {}, \
         progression {}, layers {}, tile-parts in TLM {}, PLT in first tile-part {}, GML {}, \
         tile read {}",
        width, height, across, down, layout.tile.0, layout.tile.1,
        layout.components, layout.resolutions, layout.progression, layout.layers,
        layout.tlm_parts.map_or("no TLM".to_string(), |n| n.to_string()),
        layout.plt_first.map_or("not visible", |plt| if plt { "present" } else { "absent" }),
        if gml_text(&head).is_some() { "present" } else { "absent" },
        match &index {
            Ok(index) => format!("as a TLM excerpt ({} tile-parts, per-resolution prefix {})",
                index.parts(), if index.coding().prefixed() { "possible" } else { "impossible" }),
            Err(why) => format!("by SOT walk: {}", why),
        });

    let format = match head.starts_with(CODESTREAM_MAGIC) {
        true => Format::J2k,
        false => Format::Jp2,
    };
    let placement = gml_placement(&head, width, height);
    let nodata = placement.is_some().then_some(0.0);
    let mut info = Info::plain(width, height, Kind::Jp2(Layout::of(&layout, len, format, nodata, index.ok())));
    info.placement = placement;
    Ok(info)
}

/// Georeference from the GMLJP2 box — the only place where JP2 speaks about
/// the Earth.
///
/// This is how a Sentinel-2 granule sits: an `asoc` box with a `gml.data`
/// label, inside it a second one with `gml.root-instance`, and in that a GML
/// with a rectified grid. A second way — a degenerate GeoTIFF in a `uuid` box
/// — is absent in Sentinel-2 altogether, and there is no reason to parse it
/// until a file that carries one is met.
///
/// `None` — the box is absent, the shape is wrong, or the grid is not about
/// this raster. Silence, not a refusal: JP2 without georeference is routine,
/// and such a scene falls back to the catalogue outline.
fn gml_placement(head: &[u8], width: u32, height: u32) -> Option<Placement> {
    let whole = gml_text(head)?;
    // Only the grid itself is read, not the whole document. Next to it in the
    // GML sits `gml:boundedBy` — the same `gml:pos`, but in degrees and in
    // another system — and the first occurrence over the whole text would
    // fetch that one: the scene would move a hundred kilometers away without
    // a word.
    let text = slice_of(&whole, "<gml:RectifiedGrid", "</gml:RectifiedGrid>")?;

    // The system code is taken from the start of the grid, not from anywhere:
    // srsName stands on other elements too, and a foreign one must not get in
    // here.
    let origin = slice_of(text, "<gml:origin", "</gml:origin>")?;
    let epsg = after(origin, "EPSG::")?.parse::<u32>().ok()?;
    // Zero and the user-defined code are not codes, and must not leak out:
    // `Placement` is defined by the absence of zero in it (see types.proto).
    if epsg == 0 || epsg == 32767 || !easting_first(epsg) {
        return None;
    }
    let (ox, oy) = pair(numbers(after_tag(origin, "<gml:pos>")?)?)?;

    let mut offsets = text.match_indices("<gml:offsetVector").filter_map(|(at, _)| {
        pair(numbers(after_tag(&text[at..], ">")?)?)
    });
    let (first, second) = (offsets.next()?, offsets.next()?);
    let ((x_per_i, y_per_i), (x_per_j, y_per_j)) = ordered(first, second);

    // The grid must be about this raster. Should it diverge, it describes
    // another one, and stretched over it the scene would land off itself,
    // silently at that.
    //
    // The grid bounds are read only for this check. By the letter of GML these
    // are offsets from the origin, and then at `low = 1 1` the first sample
    // would lie at `origin + v1 + v2`; the Sentinel-2 granule writes one-based
    // sample numbering there, and following the letter would shift every one
    // of its tiles by a pixel. So the origin is pinned at pixel (0, 0), and
    // `low` does not enter it.
    let (low, high) = (
        pair(numbers(after_tag(text, "<gml:low>")?)?)?,
        pair(numbers(after_tag(text, "<gml:high>")?)?)?,
    );
    let across = high.0 - low.0 + 1.0;
    let down = high.1 - low.1 + 1.0;
    if across != f64::from(width) || down != f64::from(height) {
        return None;
    }
    // A not-a-number would pass every check above: it equals nothing, itself
    // included. The warp mesh is computed from it further on, and there it can
    // no longer be caught (the same invariant is held by `tiff::usable_step`).
    if ![ox, oy, x_per_i, y_per_i, x_per_j, y_per_j].iter().all(|v| v.is_finite()) {
        return None;
    }

    // The grid origin is the center of the first sample, while the pixel
    // corner goes outward (see `Placement` in types.proto): the same
    // convention that takes half a pixel off `RasterPixelIsPoint` in GeoTIFF.
    Some(Placement {
        epsg,
        affine: [
            x_per_i,
            x_per_j,
            ox - (x_per_i + x_per_j) / 2.0,
            y_per_i,
            y_per_j,
            oy - (y_per_i + y_per_j) / 2.0,
        ],
    })
}

/// GML text from nested `asoc` boxes. Searched by content, not by order: there
/// can be several labels inside, and the needed one sits by the grid.
fn gml_text(head: &[u8]) -> Option<String> {
    fn dig(buf: &[u8], depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let mut at = 0usize;
        while at + 8 <= buf.len() {
            let length = u32::from_be_bytes(buf.get(at..at + 4)?.try_into().ok()?) as u64;
            let kind = buf.get(at + 4..at + 8)?;
            // Zero length means "to the end", one means the length follows
            // as the next eight bytes.
            let (length, body) = match length {
                0 => ((buf.len() - at) as u64, at + 8),
                1 => (u64::from_be_bytes(buf.get(at + 8..at + 16)?.try_into().ok()?), at + 16),
                _ => (length, at + 8),
            };
            let end = usize::try_from(at as u64 + length).ok()?.min(buf.len());
            if body > end {
                return None;
            }
            let inner = &buf[body..end];
            let found = match kind {
                b"asoc" => dig(inner, depth + 1),
                b"xml " => String::from_utf8(inner.to_vec())
                    .ok()
                    .filter(|text| text.contains("RectifiedGrid")),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
            at = end.max(at + 8);
        }
        None
    }
    dig(head, 0)
}

/// The tail of the string after the first occurrence — up to the end of the
/// line or up to `<`.
fn after<'a>(text: &'a str, mark: &str) -> Option<&'a str> {
    let tail = &text[text.find(mark)? + mark.len()..];
    Some(&tail[..tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len())])
}

/// The contents of an element whose opening tag was just named.
fn after_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let tail = &text[text.find(tag)? + tag.len()..];
    Some(&tail[..tail.find('<').unwrap_or(tail.len())])
}

/// A piece of text between the opening and the closing tag — to read an
/// element, not the whole document.
fn slice_of<'a>(text: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let start = text.find(from)?;
    let end = text[start..].find(to)? + start;
    Some(&text[start..end])
}

/// Whether the system's coordinates go "easting, northing" — in the order GML
/// writes them.
///
/// The order belongs to the system: UTM zones and web Mercator put easting
/// first, while Gauss-Kruger zones and local systems put northing first.
/// Swapped, it lays the scene across, and the numbers cannot reveal it: they
/// are real.
///
/// Only those whose order is known for sure are named here. The rest keep no
/// georeference from the file — and that is the right trade: the scene falls
/// back to the catalogue outline, that is, roughly, not across.
fn easting_first(epsg: u32) -> bool {
    matches!(epsg, 3857 | 32601..=32660 | 32701..=32760)
}

/// Shift vectors in the order "per column, per row".
///
/// The file sets their order with its `axisLabels`, and they write it
/// differently. A swapped pair is visible by shape: for a raster without
/// rotation the first vector runs along the row, that is, its easting
/// component is non-zero. Zero easting with non-zero northing — the axes are
/// swapped.
///
/// Only this unambiguous case is parsed. A rotated raster has both components
/// non-zero, there is nothing to guess, and the pair is taken as is.
fn ordered(first: (f64, f64), second: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    match first.0 == 0.0 && first.1 != 0.0 && second.0 != 0.0 && second.1 == 0.0 {
        true => (second, first),
        false => (first, second),
    }
}

/// Numbers separated by spaces.
fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split_whitespace().map(|word| word.parse::<f64>().ok()).collect()
}

/// Exactly two numbers: a coordinate pair or a step pair. More or fewer — the
/// shape is wrong, and there is nothing to complete it with.
fn pair(values: Vec<f64>) -> Option<(f64, f64)> {
    match values.as_slice() {
        [first, second] => Some((*first, *second)),
        _ => None,
    }
}

/// JP2 chunks behind the driver trait: a tile at the image factor, the file
/// stretch, and the "no data" key. The codec with an index is per tile: its
/// stream is the excerpt of that tile; without an index it is per factor over
/// the whole file, because the factor is set at opening and the codec's SOT
/// walk continues from the last read.
pub struct Chunks<'a> {
    resource: u64,
    layout: &'a Layout,
    bytes: Rc<Cell<u64>>,
    walker: Option<(u32, Decoder<Metered>)>,
    /// Excerpts assembled by the up-front order, by (factor, tile): their
    /// pieces have already been named to the medium in a single order for all
    /// tiles, and the codec takes the excerpt from here instead of assembling
    /// it a second time (see [`Chunked::prefetch`]).
    planned: HashMap<(u32, u32), Vec<excerpt::Segment>>,
}

impl<'a> Chunks<'a> {
    pub fn new(resource: u64, layout: &'a Layout, bytes: Rc<Cell<u64>>) -> Self {
        Self { resource, layout, bytes, walker: None, planned: HashMap::new() }
    }

    /// The excerpt of tile `tile` at factor `factor`: probes of its tile-parts
    /// are read here, the assembly is at `excerpt`. The `bytes` counter is the
    /// sum of probes and windows, while for the codec over the whole file
    /// (`Metered`) it is the farthest position reached; both enter progress as
    /// "read", and the sum exceeds the file size only when one place is read
    /// twice.
    fn assembled(&self, index: &excerpt::Index, tile: u32, factor: u32) -> Result<Vec<excerpt::Segment>, String> {
        let mut probes = Vec::new();
        for part in index.parts_of(tile) {
            let size = part.len.min(excerpt::PROBE);
            let probe = veldsdk::abi::resource_read(self.resource, part.offset, size)
                .map_err(|e| format!("jp2: tile-part probe of tile {}: {}", tile, e))?;
            self.bytes.set(self.bytes.get() + probe.len() as u64);
            probes.push(probe);
        }
        excerpt::assemble(index, tile, factor, probes).map_err(|why| format!("jp2: {}", why))
    }

    /// Tile `index` at factor `factor` — the planes and the codec header go to
    /// the reader `read` while they are alive.
    fn decode<T>(
        &mut self,
        factor: u32,
        index: u32,
        read: impl FnOnce(&Tile<'_>, Header) -> Result<T, String>,
    ) -> Result<T, String> {
        // The layout is taken by reference before the excerpt: the excerpt
        // borrows all of `self`.
        let layout = self.layout;
        match &layout.index {
            Some(excerpt) => {
                let mut decoder = self.excerpt(excerpt, index, factor)?;
                let header = decoder.header();
                decoder.tile(index, |tile| read(tile, header))
            }
            None => {
                let decoder = self.walker(factor)?;
                let header = decoder.header();
                decoder.tile(index, |tile| read(tile, header))
            }
        }
    }

    /// The codec over a tile excerpt — assembled by the up-front order or
    /// here. An excerpt assembled here names its file pieces to the medium
    /// before the codec walks them in windows: at the detailed level this is
    /// the whole tile-part, and on misses it would travel block by block.
    fn excerpt(&mut self, index: &excerpt::Index, tile: u32, factor: u32) -> Result<Decoder<excerpt::Reader>, String> {
        let segments = match self.planned.remove(&(factor, tile)) {
            Some(planned) => planned,
            None => {
                let segments = self.assembled(index, tile, factor)?;
                if let Err(why) = veldsdk::abi::resource_prefetch(self.resource, &pieces_of(&segments)) {
                    veldsdk::log::debug!(target: "decode", "jp2: pieces of the excerpt of tile {} were not prefetched: {}", tile, why);
                }
                segments
            }
        };
        let reader = excerpt::Reader::over(self.resource, segments, self.bytes.clone());
        let len = reader.len();
        opened(Decoder::open(reader, len, self.layout.format, factor))
    }

    /// The codec over the whole file at factor `factor` — ready or reopened.
    fn walker(&mut self, factor: u32) -> Result<&mut Decoder<Metered>, String> {
        if !matches!(&self.walker, Some((at, _)) if *at == factor) {
            let reader = Metered::new(self.resource, self.layout.len, self.bytes.clone());
            let decoder = opened(Decoder::open(reader, self.layout.len, self.layout.format, factor))?;
            self.walker = Some((factor, decoder));
        }
        Ok(&mut self.walker.as_mut().expect("decoder just opened").1)
    }

    /// The file stretch — ready or computed from a sample of the finest copy
    /// and remembered. Byte samples need no stretch: they have the identity and
    /// the "no data" key, and the codec is not opened for them.
    fn mapping(&mut self) -> Result<Mapping, String> {
        if let Some(ready) = *self.layout.stretch.borrow() {
            return Ok(ready);
        }
        // The layout is taken by reference before the decoder: the decoder
        // borrows all of `self`.
        let layout = self.layout;
        let nodata = layout.nodata;
        let built = match layout.precision <= 8 {
            true => Mapping::identity(nodata),
            false => {
                // The sample is four tiles scattered over the codestream grid,
                // at the finest factor, thinned to the common sampling
                // threshold; the color is taken from the first component. The
                // codec's tile grid is one for all factors and must not be
                // computed from the reduced chunk.
                let coarsest = layout.grid.overviews.len() as u32;
                let mut values = Vec::new();
                let total = layout.tiles.0 * layout.tiles.1;
                let mut picks = [0, total / 3, 2 * total / 3, total.saturating_sub(1)].to_vec();
                picks.dedup();
                for &index in &picks {
                    self.decode(coarsest, index, |tile, header| {
                        let offset = sample_offset(header);
                        let plane = tile.planes[0];
                        let step = (plane.len() * picks.len() / radiometry::STRETCH_SAMPLES).max(1);
                        for at in (0..plane.len()).step_by(step) {
                            let v = (plane[at] + offset) as f32;
                            if radiometry::is_data(v, nodata) {
                                values.push(v);
                            }
                        }
                        Ok(())
                    })?;
                }
                match percentile_stretch(&mut values) {
                    Some((lo, hi)) => Mapping::stretched(lo, hi, nodata),
                    None => Mapping::identity(nodata),
                }
            }
        };
        *layout.stretch.borrow_mut() = Some(built);
        Ok(built)
    }
}

/// An opened codec whose header unpacks into RGBA: components of one grid and
/// bit depth, from one to four.
fn opened<R: Read + Seek>(decoder: Result<Decoder<R>, String>) -> Result<Decoder<R>, String> {
    let decoder = decoder.map_err(|why| format!("jp2: {}", why))?;
    let header = decoder.header();
    if header.uneven {
        return Err("jp2: components of differing grid or bit depth cannot be laid out into RGBA".to_string());
    }
    if header.components == 0 || header.components > 4 {
        return Err(format!("jp2: {} components cannot be laid out into RGBA", header.components));
    }
    Ok(decoder)
}

impl Chunked for Chunks<'_> {
    fn chunk(&mut self, image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String> {
        let mapping = self.mapping()?;
        self.decode(image as u32, index, |tile, header| Ok((rgba(tile, header, &mapping), tile.width, tile.height)))
    }

    /// With the index the order goes in two steps: probes of tile-parts of all
    /// ordered tiles, then — with the excerpts assembled from the probes — the
    /// file pieces of all tiles in a single order. Per tile the pieces would
    /// travel request by request: at the detailed level that is one piece per
    /// tile, and fifty tiles would cost fifty latencies in a row. The assembled
    /// excerpts stay with the chunks ([`Chunks::planned`]). Without the index
    /// the codec walks on its own, and its reads cannot be predicted.
    fn prefetch(&mut self, image: usize, indices: &[u32]) -> Result<(), String> {
        let layout = self.layout;
        let Some(index) = &layout.index else { return Ok(()) };
        let probes: Vec<(u64, u64)> = indices
            .iter()
            .flat_map(|&tile| index.parts_of(tile))
            .map(|part| (part.offset, part.len.min(excerpt::PROBE)))
            .collect();
        veldsdk::abi::resource_prefetch(self.resource, &probes).map_err(|e| e.to_string())?;

        let factor = image as u32;
        let mut pieces = Vec::new();
        let mut assembled = Vec::with_capacity(indices.len());
        for &tile in indices {
            let segments = self.assembled(index, tile, factor)?;
            pieces.extend(pieces_of(&segments));
            assembled.push((tile, segments));
        }
        // The excerpts stay even without the fetched pieces — misses will read
        // them; while a failed assembly leaves not one, and the tiles assemble
        // their own.
        let ordered = veldsdk::abi::resource_prefetch(self.resource, &pieces).map_err(|e| e.to_string());
        for (tile, segments) in assembled {
            self.planned.insert((factor, tile), segments);
        }
        ordered
    }
}

/// File pieces in an excerpt — what is named to the medium up front.
fn pieces_of(segments: &[excerpt::Segment]) -> Vec<(u64, u64)> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            excerpt::Segment::File { offset, len } => Some((*offset, *len)),
            excerpt::Segment::Bytes(_) => None,
        })
        .collect()
}

/// The shift that turns a signed sample into an unsigned one of the same bit
/// depth.
fn sample_offset(header: Header) -> i32 {
    match header.signed {
        true => 1 << header.precision.saturating_sub(1).min(30),
        false => 0,
    }
}

/// Tile planes into RGBA by the pixel layout: samples up to a byte are extended
/// to a byte by a shift, wider ones go through the file stretch.
fn rgba(tile: &Tile<'_>, header: Header, mapping: &Mapping) -> Vec<u8> {
    let pixels = (tile.width as usize) * (tile.height as usize);
    let channels = tile.planes.len();
    let pixel = Pixel::named(channels);
    let offset = sample_offset(header);
    if header.precision <= 8 {
        let top = (1i32 << header.precision) - 1;
        let shift = 8 - header.precision;
        let mut samples = vec![0u8; pixels * channels];
        for (c, plane) in tile.planes.iter().enumerate() {
            for (at, value) in plane.iter().take(pixels).enumerate() {
                samples[at * channels + c] = (((value + offset).clamp(0, top)) << shift) as u8;
            }
        }
        return mapping.rgba(&Samples::U8(&samples), pixel, pixels);
    }
    let mut samples = vec![0u16; pixels * channels];
    for (c, plane) in tile.planes.iter().enumerate() {
        for (at, value) in plane.iter().take(pixels).enumerate() {
            samples[at * channels + c] = (value + offset).clamp(0, i32::from(u16::MAX)) as u16;
        }
    }
    mapping.rgba(&Samples::U16(&samples), pixel, pixels)
}

/// Point-wise reading of level tiles — by the driver over codestream tiles.
pub fn produce_direct(
    resource: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::new(resource, layout, bytes.clone());
    grid::direct(&mut chunks, &layout.grid, (info.width, info.height), level, wants, emit)
}

/// A pass over codestream tiles at native resolution — by the driver.
pub fn produce_pass(
    resource: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::new(resource, layout, bytes.clone());
    grid::pass(&mut chunks, &layout.grid, (info.width, info.height), emit)
}

// ── Header parsing without the decoder ──────────────────────────

/// JP2 container signature (signature box) and raw codestream (SOC+SIZ).
pub const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
pub const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";

/// Where to step past a box of length `len` that started at `at`.
///
/// Computed in `u64` and clamped to the head length, and both halves are
/// required. The box length is written by the file, and files are sometimes
/// broken: declaring four gigabytes, it moves the position to the very end of
/// the address space — in the module it is thirty-two-bit. Then `at + 8`
/// wraps in the loop condition, silently (release builds have no overflow
/// checks), and the pass enters the body with a position past the end of the
/// slice — that is, the slice backwards, that is, a trap for the whole
/// instance.
///
/// The clamp also ends the walk: a step past the head edge does not pass the
/// loop.
fn step(at: usize, len: u64, limit: usize) -> usize {
    (at as u64).saturating_add(len).min(limit as u64) as usize
}

/// Raster size from the file head: for a container — box `jp2h`→`ihdr`, for a
/// raw codestream — marker SIZ. Failure means the header is broken or did not
/// fit into the head that was read.
pub fn header_dims(head: &[u8]) -> Result<(u32, u32), String> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return siz_dims(&head[2..]);
    }
    if !head.starts_with(JP2_MAGIC) {
        return Err("jp2: no signature".to_string());
    }

    // Top-level boxes: [length u32][type 4B][body]. Length 1 is extended
    // (the next 8 bytes), 0 runs to the end of the file.
    let mut at = 0usize;
    while at + 8 <= head.len() {
        let len = u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as u64;
        let kind = &head[at + 4..at + 8];
        let (body, next) = match len {
            0 => (at + 8, head.len()),
            1 => {
                if at + 16 > head.len() {
                    break;
                }
                let xlen = u64::from_be_bytes(head[at + 8..at + 16].try_into().unwrap());
                (at + 16, step(at, xlen, head.len()))
            }
            _ => (at + 8, step(at, len, head.len())),
        };
        if kind == b"jp2h" {
            // Inside the superbox ihdr must come first (ISO 15444-1, I.5.3):
            // [length][ihdr][height u32][width u32][channels u16]…
            let ihdr = body;
            if ihdr + 16 > head.len() || &head[ihdr + 4..ihdr + 8] != b"ihdr" {
                return Err("jp2: jp2h without ihdr as its first box".to_string());
            }
            let height = u32::from_be_bytes(head[ihdr + 8..ihdr + 12].try_into().unwrap());
            let width = u32::from_be_bytes(head[ihdr + 12..ihdr + 16].try_into().unwrap());
            return Ok((width, height));
        }
        if next <= at {
            break;
        }
        at = next;
    }
    Err("jp2: header not found in the file head".to_string())
}

/// SIZ: [marker FF51][Lsiz u16][Rsiz u16][Xsiz u32][Ysiz u32][XOsiz][YOsiz]…
/// The raster size is the canvas area minus the offset.
fn siz_dims(from_siz: &[u8]) -> Result<(u32, u32), String> {
    if from_siz.len() < 22 || from_siz[0] != 0xFF || from_siz[1] != 0x51 {
        return Err("jp2: codestream without SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(from_siz[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, xo, yo) = (be32(6), be32(10), be32(14), be32(18));
    if xsiz <= xo || ysiz <= yo {
        return Err("jp2: empty canvas in SIZ".to_string());
    }
    Ok((xsiz - xo, ysiz - yo))
}

/// Codestream layout: how it is cut up and how many resolutions it is laid out
/// in.
///
/// Not needed for display — it is a measure of the cost of reading a piece,
/// and today only the description log prints it (see [`describe`]).
/// The unit of reading in JPEG 2000 is the **tile-part**, not the packet: the
/// tile body is read as one piece. So "read a piece of the picture" costs
/// exactly as many tiles as the piece touches, and for a file cut into one
/// tile the choice of piece saves not a byte.
///
/// `resolutions` — how many wavelet stages are written. The decoder does not
/// give anything coarser than the last one, and such pyramid levels are filled
/// in by halving on our side.
///
/// Three fields about whether a tile can be found without walking the whole
/// file: `tlm_parts` — how many tile-parts are listed in the main header TLM
/// (`None` — there is no TLM, and the tile address is learned only by walking
/// SOT); `plt_first` — whether PLT is in the header of the first tile-part
/// (`None` — it did not fit into the head); `progression` — the progression
/// order from COD. They are written to the description log: by them it is
/// decided how to read the granule over the network (see
/// `docs/decisions/0002`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codestream {
    /// Canvas and its origin (`Xsiz`/`Ysiz`, `XOsiz`/`YOsiz`).
    pub canvas: (u32, u32),
    pub origin: (u32, u32),
    /// Tile grid step and its origin (`XTsiz`/`YTsiz`, `XTOsiz`/`YTOsiz`).
    pub tile: (u32, u32),
    pub tile_origin: (u32, u32),
    pub components: u16,
    /// Bit depth of the first component (`Ssiz`): up to a byte, samples go
    /// without a stretch.
    pub precision: u8,
    pub resolutions: u8,
    pub progression: u8,
    /// Quality layers from COD: with a single layer the tile-part packets in
    /// LRCP go by resolutions, and its prefix is the coarse levels.
    pub layers: u16,
    pub tlm_parts: Option<u32>,
    pub plt_first: Option<bool>,
}

impl Codestream {
    /// Tiles across and down (ISO 15444-1, B.3).
    pub fn grid(&self) -> (u32, u32) {
        let across = |canvas: u32, origin: u32, step: u32| match step {
            0 => 0,
            _ => (canvas.saturating_sub(origin)).div_ceil(step),
        };
        (
            across(self.canvas.0, self.tile_origin.0, self.tile.0),
            across(self.canvas.1, self.tile_origin.1, self.tile.1),
        )
    }
}

/// Where the codestream lies in a file of length `len`: its start in the head
/// and its end in the file. For a bare codestream this is the whole file, for
/// a container — the body of the `jp2c` box, and its zero length means "to the
/// end of the file". `None` — the box did not fit into the head that was read.
pub(super) fn codestream_span(head: &[u8], len: u64) -> Option<(usize, u64)> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return Some((0, len));
    }
    let mut at = 0usize;
    while at + 8 <= head.len() {
        let declared = u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as u64;
        let kind = &head[at + 4..at + 8];
        let (body, declared) = match declared {
            0 => (at + 8, 0),
            1 => {
                if at + 16 > head.len() {
                    return None;
                }
                (at + 16, u64::from_be_bytes(head[at + 8..at + 16].try_into().unwrap()))
            }
            _ => (at + 8, declared),
        };
        if kind == b"jp2c" {
            let end = match declared {
                0 => len,
                _ => (at as u64).saturating_add(declared).min(len),
            };
            return Some((body, end));
        }
        let next = match declared {
            0 => head.len(),
            _ => step(at, declared, head.len()),
        };
        if next <= at {
            return None;
        }
        at = next;
    }
    None
}

/// A walk over main header segments past SIZ: the marker and the segment body
/// (without the marker and the length), until SOT is met. One for the layout
/// ([`codestream`]) and the tile-part index (`excerpt::Index`): two walkers of
/// one header have no reason to disagree about where it ends.
///
/// The length of a segment stands right after the marker and counts itself, so
/// the next marker is `2 + L` further. A segment that did not fit into the
/// head ends the walk silently: the head is finite, and this is not file
/// corruption.
pub(super) struct Segments<'a> {
    cs: &'a [u8],
    walk: usize,
    sot: Option<usize>,
}

pub(super) fn segments(cs: &[u8]) -> Segments<'_> {
    // SOC is two bytes without a length, then SIZ with its length behind the
    // marker.
    let lsiz = cs.get(4..6).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
    Segments { cs, walk: 4 + lsiz, sot: None }
}

impl<'a> Segments<'a> {
    /// The next segment; `None` — SOT was reached or the head ran out.
    /// A non-marker where a marker belongs is a failure: the header is broken.
    pub(super) fn next(&mut self) -> Result<Option<(u8, &'a [u8])>, String> {
        let cs = self.cs;
        if self.sot.is_some() || self.walk + 4 > cs.len() {
            return Ok(None);
        }
        if cs[self.walk] != 0xFF {
            return Err("jp2: main header ended not on a marker".to_string());
        }
        let marker = cs[self.walk + 1];
        if marker == 0x90 {
            self.sot = Some(self.walk);
            return Ok(None);
        }
        let length = usize::from(u16::from_be_bytes([cs[self.walk + 2], cs[self.walk + 3]]));
        if length < 2 || self.walk + 2 + length > cs.len() {
            self.walk = cs.len();
            return Ok(None);
        }
        let body = &cs[self.walk + 4..self.walk + 2 + length];
        self.walk += 2 + length;
        Ok(Some((marker, body)))
    }

    /// Where the first SOT stands, if the walk reached it.
    pub(super) fn sot(&self) -> Option<usize> {
        self.sot
    }
}

/// Layout from the head: marker SIZ gives the canvas and the tile grid, the
/// first COD — the resolution count.
///
/// COD is searched by walking markers, not by offset: optional segments stand
/// between SIZ and COD, and the file chooses their order. The walk ends at SOT
/// — tiles follow there, while we need the main header.
pub fn codestream(head: &[u8]) -> Result<Codestream, String> {
    let (at, _) = codestream_span(head, head.len() as u64).ok_or("jp2: codestream not found in the file head")?;
    let cs = &head[at..];
    if cs.len() < 43 || cs[0] != 0xFF || cs[1] != 0x4F || cs[2] != 0xFF || cs[3] != 0x51 {
        return Err("jp2: codestream without SOC and SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let be16 = |at: usize| u16::from_be_bytes(cs[at..at + 2].try_into().unwrap());
    // SOC is two bytes without a length, then the SIZ segment: [marker 2]
    // [Lsiz 2][Rsiz 2][Xsiz 4][Ysiz 4][XOsiz 4][YOsiz 4][XTsiz 4][YTsiz 4]
    // [XTOsiz 4][YTOsiz 4][Csiz 2][Ssiz 1 XRsiz 1 YRsiz 1]… Offsets count from
    // the SIZ marker, not from the codestream start: the SOC before it takes
    // its two bytes. In Ssiz the low seven bits are the bit depth minus one.
    let siz = 2usize;
    let mut layout = Codestream {
        canvas: (be32(siz + 6), be32(siz + 10)),
        origin: (be32(siz + 14), be32(siz + 18)),
        tile: (be32(siz + 22), be32(siz + 26)),
        tile_origin: (be32(siz + 30), be32(siz + 34)),
        components: be16(siz + 38),
        precision: (cs[siz + 40] & 0x7F).saturating_add(1),
        resolutions: 0,
        progression: 0,
        layers: 0,
        tlm_parts: None,
        plt_first: None,
    };

    let mut walk = segments(cs);
    while let Some((marker, body)) = walk.next()? {
        // COD: [Scod 1][SGcod: order 1, layers 2, MCT 1][SPcod: stages 1…].
        // Wavelet stages are one less than the resolution count.
        if marker == 0x52 && body.len() >= 10 {
            layout.progression = body[1];
            layout.layers = u16::from_be_bytes([body[2], body[3]]);
            layout.resolutions = body[5].saturating_add(1);
        }
        // TLM: [Ztlm 1][Stlm 1][records…]; in Stlm bits 4–5 are the width of
        // Ttlm (0, 1 or 2 bytes), bit 6 is the width of Ptlm (2 or 4). There
        // can be several segments, and the records are counted over all of
        // them.
        if marker == 0x55 && body.len() >= 2 {
            let stlm = body[1];
            let entry = usize::from((stlm >> 4) & 3) + if stlm & 0x40 != 0 { 4 } else { 2 };
            let listed = ((body.len() - 2) / entry) as u32;
            layout.tlm_parts = Some(layout.tlm_parts.unwrap_or(0) + listed);
        }
    }
    if let Some(sot) = walk.sot() {
        layout.plt_first = first_tile_part_has_plt(cs, sot);
    }
    if layout.resolutions == 0 {
        return Err("jp2: codestream without COD in the file head".to_string());
    }
    Ok(layout)
}

/// Whether PLT is in the header of the first tile-part, which starts with
/// marker SOT at `at`. `None` — the header did not fit into the head that was
/// read: SOD was not reached and PLT was not met.
fn first_tile_part_has_plt(cs: &[u8], at: usize) -> Option<bool> {
    // SOT: [Lsot 2 = 10][Isot 2][Psot 4][TPsot 1][TNsot 1]; then the tile-part
    // header segments up to SOD.
    let mut walk = at + 12;
    while walk + 2 <= cs.len() && cs[walk] == 0xFF {
        match cs[walk + 1] {
            0x93 => return Some(false),
            0x58 => return Some(true),
            _ => {}
        }
        // Other segments carry a length; SOD and PLT are decided above, before
        // it.
        if walk + 4 > cs.len() {
            break;
        }
        let length = usize::from(u16::from_be_bytes([cs[walk + 2], cs[walk + 3]]));
        if length < 2 {
            break;
        }
        walk += 2 + length;
    }
    None
}
