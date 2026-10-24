use std::cell::RefCell;
use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use super::super::budget::Peak;
use super::super::cascade::Emit;
use super::grid::{self, Chunked, Grid, Overview};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples};
use super::{placed, Info, Kind, Placement, Tie};

pub const BIG_MAGIC: [&[u8]; 2] = [b"II\x2b\x00", b"MM\x00\x2b"];

pub struct Layout {
    pub grid: Grid,
    /// The file's single stretch: computed from the smallest copy or from base
    /// chunks alike, it must not vary per request — neighbouring tiles would
    /// come out with different brightness, and what lands in the disk cache
    /// would depend on request order.
    stretch: RefCell<Option<Mapping>>,
}

impl Layout {
    pub fn of(tiled: bool, chunk: (u32, u32), overviews: Vec<Overview>, depth: u32) -> Self {
        Self { grid: Grid { tiled, chunk, overviews, depth }, stretch: RefCell::new(None) }
    }

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
        let chunk = chunk_grid(&mut decoder).unwrap_or((w, h));
        overviews.push(Overview { image: index, width: w, height: h, chunk });
    }

    Ok(Info {
        width,
        height,
        kind: Kind::Tiff(Layout::of(tiled, chunk, overviews, depth)),
        ties,
        placement,
        frame: None,
        binding_trouble,
        variable: None,
        variables: Vec::new(),
    })
}

fn is_overview(subfile_type: u32) -> bool {
    const REDUCED: u32 = 1;
    const MASK: u32 = 4;
    subfile_type & REDUCED != 0 && subfile_type & MASK == 0
}

fn georef<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    width: u32,
    height: u32,
) -> (Vec<Tie>, Option<Placement>, Option<String>) {
    let keys = decoder.get_tag_u16_vec(Tag::GeoKeyDirectoryTag).unwrap_or_default();
    let points = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).unwrap_or_default();
    let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).unwrap_or_default();
    let matrix = decoder.get_tag_f64_vec(Tag::Unknown(34264)).unwrap_or_default();

    if let Some(code) = foreign_datum(&keys) {
        veldsdk::log::warn!(target: "decode",
            "the raster declares datum EPSG:{}, but the binding lands as WGS84: a shift of about a hundred metres",
            code);
    }
    let placement = geo_placement(&keys, &points, &scale, &matrix, width, height);
    let ties = geo_ties(&keys, &points, &scale, &matrix, width, height);
    let carries = !points.is_empty() || !scale.is_empty() || !matrix.is_empty();
    let taken = !ties.is_empty() || placement.is_some();
    let trouble = binding_trouble(carries, taken, &keys, points.len() / 6);
    if let Some(said) = &trouble {
        veldsdk::log::warn!(target: "decode", "{}", said);
    }
    (ties, placement, trouble)
}

fn binding_trouble(carries: bool, taken: bool, keys: &[u16], points: usize) -> Option<String> {
    (carries && !taken).then(|| {
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

fn geokey(keys: &[u16], id: u16) -> Option<u16> {
    keys.get(4..)
        .unwrap_or_default()
        .chunks_exact(4)
        .find(|entry| entry[0] == id && entry[1] == 0)
        .map(|entry| entry[3])
}

// GeoKey 1025 = 2 is RasterPixelIsPoint: the coordinates name pixel centers,
// while the binding counts corners, so everything moves by half a pixel. The
// spec default is the corner and needs no shift.
fn half_pixel(keys: &[u16]) -> f64 {
    if geokey(keys, 1025) == Some(2) { 0.5 } else { 0.0 }
}

fn foreign_datum(keys: &[u16]) -> Option<u16> {
    const WGS84: [u16; 3] = [4326, 4979, 4055];
    const UNSAID: [u16; 2] = [0, 32767];
    geokey(keys, 2048).filter(|code| !WGS84.contains(code) && !UNSAID.contains(code))
}

fn usable_step(scale: &[f64]) -> bool {
    matches!(scale.get(..2), Some([x, y]) if x.is_finite() && y.is_finite() && *x != 0.0 && *y != 0.0)
}

fn geo_placement(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
    width: u32,
    height: u32,
) -> Option<Placement> {
    if geokey(keys, 1024) != Some(1) || points.len() > 6 {
        return None;
    }
    let epsg = match geokey(keys, 3072) {
        Some(code) if code != 0 && code != 32767 => u32::from(code),
        _ => return None,
    };

    let half = half_pixel(keys);
    let affine = match (points.get(..6), usable_step(scale)) {
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
    let (right, bottom) = (f64::from(width), f64::from(height));
    let corner = [
        affine[0] * right + affine[1] * bottom + affine[2],
        affine[3] * right + affine[4] * bottom + affine[5],
    ];
    let whole = affine.iter().chain(corner.iter()).all(|value| value.is_finite());
    whole.then_some(Placement { epsg, affine })
}

fn geo_ties(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
    width: u32,
    height: u32,
) -> Vec<Tie> {
    if geokey(keys, 1024) != Some(2) {
        return Vec::new();
    }
    let half = half_pixel(keys);

    let point = |px: f64, py: f64, lon: f64, lat: f64| Tie { px, py, lat, lon };
    if points.len() > 6 {
        let ties: Vec<Tie> = points
            .chunks_exact(6)
            .map(|tie| point(tie[0] + half, tie[1] + half, tie[3], tie[4]))
            .collect();
        return match ties.iter().all(|tie| placed(tie.lat, tie.lon)) {
            true => finite(ties),
            false => Vec::new(),
        };
    }

    let (Some(tie), true) = (points.get(..6), usable_step(scale)) else {
        return finite(corners_from_matrix(matrix, half, width, height));
    };
    let (x, y) = (tie[3] - (tie[0] + half) * scale[0], tie[4] + (tie[1] + half) * scale[1]);
    let (right, bottom) = (f64::from(width), f64::from(height));
    finite(vec![
        point(0.0, 0.0, x, y),
        point(right, 0.0, x + right * scale[0], y),
        point(0.0, bottom, x, y - bottom * scale[1]),
        point(right, bottom, x + right * scale[0], y - bottom * scale[1]),
    ])
}

fn finite(ties: Vec<Tie>) -> Vec<Tie> {
    let whole = |tie: &Tie| {
        tie.px.is_finite() && tie.py.is_finite() && tie.lat.is_finite() && tie.lon.is_finite()
    };
    match ties.iter().all(whole) {
        true => ties,
        false => Vec::new(),
    }
}

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

struct Chunks<R: Read + Seek> {
    decoder: Decoder<R>,
    mapping: Mapping,
    at: Option<(usize, Pixel)>,
    resource: u64,
}

impl<R: Read + Seek> Chunks<R> {
    fn new(reader: R, resource: u64, layout: &Layout) -> Result<Self, String> {
        let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
        let mapping = stretch(layout, &mut decoder)?;
        Ok(Self { decoder, mapping, at: None, resource })
    }

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

pub fn produce_pass<R: Read + Seek>(reader: R, resource: u64, info: &Info, layout: &Layout, emit: Emit) -> Result<(), String> {
    let mut chunks = Chunks::new(reader, resource, layout)?;
    grid::pass(&mut chunks, &layout.grid, (info.width, info.height), emit)
}

fn chunk_grid<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(u32, u32), String> {
    ensure_chunky(decoder)?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: zero chunk size".to_string());
    }
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

fn model(color: tiff::ColorType) -> Result<(Pixel, u8), String> {
    match color {
        tiff::ColorType::Gray(bits) => Ok((Pixel::named(1), bits)),
        tiff::ColorType::GrayA(bits) => Ok((Pixel::named(2), bits)),
        tiff::ColorType::RGB(bits) => Ok((Pixel::named(3), bits)),
        tiff::ColorType::RGBA(bits) => Ok((Pixel::named(4), bits)),
        tiff::ColorType::Multiband { bit_depth, num_samples } => {
            Ok((Pixel::stack(usize::from(num_samples)), bit_depth))
        }
        other => Err(format!("tiff: color model {:?} is not supported", other)),
    }
}

fn pixel(color: tiff::ColorType) -> Result<Pixel, String> {
    Ok(model(color)?.0)
}

fn depth_of<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<u32, String> {
    let (pixel, bits) = model(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
    Ok(pixel.channels as u32 * u32::from(bits.div_ceil(8)))
}

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

fn ensure_decodable<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
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

fn decodable(code: u16) -> bool {
    DECODED.iter().any(|(known, _)| *known == code)
}

fn sampled(format: u16, bits: u8) -> bool {
    matches!((format, bits), (1, 8) | (1, 16) | (2, 16) | (3, 32))
}

fn ensure_sampled<R: Read + Seek>(decoder: &mut Decoder<R>, bits: u8) -> Result<(), String> {
    let format = decoder
        .get_tag_u16_vec(Tag::SampleFormat)
        .ok()
        .and_then(|formats| formats.first().copied())
        .or_else(|| decoder.get_tag_unsigned::<u16>(Tag::SampleFormat).ok())
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

fn stretch<R: Read + Seek>(layout: &Layout, decoder: &mut Decoder<R>) -> Result<Mapping, String> {
    if let Some(ready) = *layout.stretch.borrow() {
        return Ok(ready);
    }
    let built = mapping(decoder, layout.stats())?;
    *layout.stretch.borrow_mut() = Some(built);
    Ok(built)
}

fn mapping<R: Read + Seek>(decoder: &mut Decoder<R>, stats: usize) -> Result<Mapping, String> {
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
        None => Ok(Mapping::identity(nodata)),
    }
}
