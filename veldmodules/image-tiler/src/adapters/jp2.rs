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

const HEAD: usize = 64 * 1024;

const SAMPLE_BYTES: u32 = 8;

pub struct Layout {
    pub grid: Grid,
    pub tiles: (u32, u32),
    pub len: u64,
    format: Format,
    precision: u8,
    nodata: Option<f32>,
    stretch: RefCell<Option<Mapping>>,
    index: Option<excerpt::Index>,
}

impl Layout {
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

    pub fn indexed(&self) -> bool {
        self.index.is_some()
    }
}

pub fn describe(mut reader: Metered, len: u64) -> Result<Info, String> {
    let mut head = vec![0u8; len.min(HEAD as u64) as usize];
    reader.read_exact(&mut head).map_err(|e| format!("jp2: header read: {}", e))?;
    let (width, height) = header_dims(&head)?;
    let layout = codestream(&head)?;
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

fn gml_placement(head: &[u8], width: u32, height: u32) -> Option<Placement> {
    let whole = gml_text(head)?;
    let text = slice_of(&whole, "<gml:RectifiedGrid", "</gml:RectifiedGrid>")?;

    let origin = slice_of(text, "<gml:origin", "</gml:origin>")?;
    let epsg = after(origin, "EPSG::")?.parse::<u32>().ok()?;
    if epsg == 0 || epsg == 32767 || !easting_first(epsg) {
        return None;
    }
    let (ox, oy) = pair(numbers(after_tag(origin, "<gml:pos>")?)?)?;

    let mut offsets = text.match_indices("<gml:offsetVector").filter_map(|(at, _)| {
        pair(numbers(after_tag(&text[at..], ">")?)?)
    });
    let (first, second) = (offsets.next()?, offsets.next()?);
    let ((x_per_i, y_per_i), (x_per_j, y_per_j)) = ordered(first, second);

    let (low, high) = (
        pair(numbers(after_tag(text, "<gml:low>")?)?)?,
        pair(numbers(after_tag(text, "<gml:high>")?)?)?,
    );
    let across = high.0 - low.0 + 1.0;
    let down = high.1 - low.1 + 1.0;
    if across != f64::from(width) || down != f64::from(height) {
        return None;
    }
    if ![ox, oy, x_per_i, y_per_i, x_per_j, y_per_j].iter().all(|v| v.is_finite()) {
        return None;
    }

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

fn gml_text(head: &[u8]) -> Option<String> {
    fn dig(buf: &[u8], depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let mut at = 0usize;
        while at + 8 <= buf.len() {
            let length = u32::from_be_bytes(buf.get(at..at + 4)?.try_into().ok()?) as u64;
            let kind = buf.get(at + 4..at + 8)?;
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

fn after<'a>(text: &'a str, mark: &str) -> Option<&'a str> {
    let tail = &text[text.find(mark)? + mark.len()..];
    Some(&tail[..tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len())])
}

fn after_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let tail = &text[text.find(tag)? + tag.len()..];
    Some(&tail[..tail.find('<').unwrap_or(tail.len())])
}

fn slice_of<'a>(text: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let start = text.find(from)?;
    let end = text[start..].find(to)? + start;
    Some(&text[start..end])
}

fn easting_first(epsg: u32) -> bool {
    matches!(epsg, 3857 | 32601..=32660 | 32701..=32760)
}

fn ordered(first: (f64, f64), second: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    match first.0 == 0.0 && first.1 != 0.0 && second.0 != 0.0 && second.1 == 0.0 {
        true => (second, first),
        false => (first, second),
    }
}

fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split_whitespace().map(|word| word.parse::<f64>().ok()).collect()
}

fn pair(values: Vec<f64>) -> Option<(f64, f64)> {
    match values.as_slice() {
        [first, second] => Some((*first, *second)),
        _ => None,
    }
}

pub struct Chunks<'a> {
    resource: u64,
    layout: &'a Layout,
    bytes: Rc<Cell<u64>>,
    walker: Option<(u32, Decoder<Metered>)>,
    planned: HashMap<(u32, u32), Vec<excerpt::Segment>>,
}

impl<'a> Chunks<'a> {
    pub fn new(resource: u64, layout: &'a Layout, bytes: Rc<Cell<u64>>) -> Self {
        Self { resource, layout, bytes, walker: None, planned: HashMap::new() }
    }

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

    fn decode<T>(
        &mut self,
        factor: u32,
        index: u32,
        read: impl FnOnce(&Tile<'_>, Header) -> Result<T, String>,
    ) -> Result<T, String> {
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

    fn walker(&mut self, factor: u32) -> Result<&mut Decoder<Metered>, String> {
        if !matches!(&self.walker, Some((at, _)) if *at == factor) {
            let reader = Metered::new(self.resource, self.layout.len, self.bytes.clone());
            let decoder = opened(Decoder::open(reader, self.layout.len, self.layout.format, factor))?;
            self.walker = Some((factor, decoder));
        }
        Ok(&mut self.walker.as_mut().expect("decoder just opened").1)
    }

    fn mapping(&mut self) -> Result<Mapping, String> {
        if let Some(ready) = *self.layout.stretch.borrow() {
            return Ok(ready);
        }
        let layout = self.layout;
        let nodata = layout.nodata;
        let built = match layout.precision <= 8 {
            true => Mapping::identity(nodata),
            false => {
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
        let ordered = veldsdk::abi::resource_prefetch(self.resource, &pieces).map_err(|e| e.to_string());
        for (tile, segments) in assembled {
            self.planned.insert((factor, tile), segments);
        }
        ordered
    }
}

fn pieces_of(segments: &[excerpt::Segment]) -> Vec<(u64, u64)> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            excerpt::Segment::File { offset, len } => Some((*offset, *len)),
            excerpt::Segment::Bytes(_) => None,
        })
        .collect()
}

fn sample_offset(header: Header) -> i32 {
    match header.signed {
        true => 1 << header.precision.saturating_sub(1).min(30),
        false => 0,
    }
}

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

pub const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
pub const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";

fn step(at: usize, len: u64, limit: usize) -> usize {
    (at as u64).saturating_add(len).min(limit as u64) as usize
}

pub fn header_dims(head: &[u8]) -> Result<(u32, u32), String> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return siz_dims(&head[2..]);
    }
    if !head.starts_with(JP2_MAGIC) {
        return Err("jp2: no signature".to_string());
    }

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codestream {
    pub canvas: (u32, u32),
    pub origin: (u32, u32),
    pub tile: (u32, u32),
    pub tile_origin: (u32, u32),
    pub components: u16,
    pub precision: u8,
    pub resolutions: u8,
    pub progression: u8,
    pub layers: u16,
    pub tlm_parts: Option<u32>,
    pub plt_first: Option<bool>,
}

impl Codestream {
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

pub(super) struct Segments<'a> {
    cs: &'a [u8],
    walk: usize,
    sot: Option<usize>,
}

pub(super) fn segments(cs: &[u8]) -> Segments<'_> {
    let lsiz = cs.get(4..6).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
    Segments { cs, walk: 4 + lsiz, sot: None }
}

impl<'a> Segments<'a> {
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

    pub(super) fn sot(&self) -> Option<usize> {
        self.sot
    }
}

pub fn codestream(head: &[u8]) -> Result<Codestream, String> {
    let (at, _) = codestream_span(head, head.len() as u64).ok_or("jp2: codestream not found in the file head")?;
    let cs = &head[at..];
    if cs.len() < 43 || cs[0] != 0xFF || cs[1] != 0x4F || cs[2] != 0xFF || cs[3] != 0x51 {
        return Err("jp2: codestream without SOC and SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let be16 = |at: usize| u16::from_be_bytes(cs[at..at + 2].try_into().unwrap());
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
        if marker == 0x52 && body.len() >= 10 {
            layout.progression = body[1];
            layout.layers = u16::from_be_bytes([body[2], body[3]]);
            layout.resolutions = body[5].saturating_add(1);
        }
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

fn first_tile_part_has_plt(cs: &[u8], at: usize) -> Option<bool> {
    let mut walk = at + 12;
    while walk + 2 <= cs.len() && cs[walk] == 0xFF {
        match cs[walk + 1] {
            0x93 => return Some(false),
            0x58 => return Some(true),
            _ => {}
        }
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
