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

pub const MAX_SOURCE_SIDE: u32 = 65_536;

pub const FULL_DECODE_BUDGET: u64 = 256 * 1024 * 1024;

pub fn frame_fits(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) * 4 <= FULL_DECODE_BUDGET
}

pub struct Info {
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
    pub ties: Vec<Tie>,
    pub placement: Option<Placement>,
    pub frame: Option<netcdf::Frame>,
    pub binding_trouble: Option<String>,
    pub variable: Option<Variable>,
    pub variables: Vec<Variable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub path: String,
    pub said: String,
    pub units: String,
}

pub struct Tie {
    pub px: f64,
    pub py: f64,
    pub lat: f64,
    pub lon: f64,
}

pub fn placed(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat) && (-360.0..=360.0).contains(&lon)
}

pub struct Placement {
    pub epsg: u32,
    pub affine: [f64; 6],
}

pub enum Kind {
    Png { interlaced: bool },
    Jpeg,
    Jp2(jp2::Layout),
    Tiff(tiff::Layout),
    Full(ImageFormat),
    Netcdf(netcdf::Layout),
}

impl Info {
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

pub fn describe(resource_id: u64, len: u64, wanted: &str, bytes: &Rc<Cell<u64>>) -> Result<Info, String> {
    let mut reader = Metered::new(resource_id, len, bytes.clone());
    let mut head = [0u8; 32];
    let read = reader.read(&mut head).map_err(|e| format!("reading the header: {}", e))?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    if !wanted.is_empty() && !head.starts_with(netcdf::MAGIC) {
        return Err(format!("quantity '{}' requested of a single-quantity file", wanted));
    }

    if head.starts_with(jp2::JP2_MAGIC) || head.starts_with(jp2::CODESTREAM_MAGIC) {
        let info = jp2::describe(reader, len)?;
        return checked(info);
    }
    if head.starts_with(netcdf::MAGIC) {
        drop(reader);
        let info = netcdf::describe(resource_id, len, wanted)?;
        return checked(info);
    }
    if netcdf::CLASSIC.iter().any(|magic| head.starts_with(magic)) {
        return Err("this is classic NetCDF-3, while what is read is NetCDF-4 — the one written as HDF5"
            .to_string());
    }
    if tiff::BIG_MAGIC.iter().any(|magic| head.starts_with(magic)) {
        let info = tiff::describe(reader)?;
        return checked(info);
    }

    let format = image::guess_format(&head[..read]).map_err(|_| {
        "by the header this is not an image \
         (opened are PNG, JPEG, TIFF, JPEG 2000, NetCDF, GIF, BMP, WebP)"
            .to_string()
    })?;

    let info = match format {
        ImageFormat::Png => png::describe(reader),
        ImageFormat::Jpeg => jpeg::describe(reader),
        ImageFormat::Tiff => tiff::describe(reader),
        ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP => full::describe(reader, format),
        other => Err(format!("format {:?} is not supported", other)),
    }?;
    checked(info)
}

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
    let rows = info.levels();
    if let Some(top) = rows.last()
        && rows.iter().all(|row| !row.fits)
    {
        return Err(format!("no level fits into memory: {}", top.peak.note()));
    }
    Ok(info)
}

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
        (Kind::Tiff(layout), table::Serve::Pointwise) => {
            tiff::produce_direct(reader(), resource_id, info, layout, level, wants, emit)
        }
        (Kind::Tiff(layout), table::Serve::Pass { .. }) => {
            tiff::produce_pass(reader(), resource_id, info, layout, emit)
        }
        (Kind::Jp2(layout), table::Serve::Pointwise) => {
            jp2::produce_direct(resource_id, bytes, info, layout, level, wants, emit)
        }
        (Kind::Jp2(layout), table::Serve::Pass { .. }) => jp2::produce_pass(resource_id, bytes, info, layout, emit),
        (Kind::Netcdf(layout), table::Serve::Pointwise) => {
            netcdf::produce_direct(resource_id, len, bytes, info, layout, level, wants, emit)
        }
        (Kind::Netcdf(layout), table::Serve::Pass { .. }) => {
            netcdf::produce_pass(resource_id, len, bytes, info, layout, emit)
        }
        (Kind::Png { .. }, _) => png::produce_pass(reader(), info, emit),
        (Kind::Jpeg, _) => jpeg::produce(reader(), info, level, emit),
        (Kind::Full(format), _) => full::produce(reader(), info, *format, emit),
    }
}

const BLEED_STEPS: u32 = 2;

pub fn bleed_alpha(rgba: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return;
    }
    let mut coloured: Vec<bool> = (0..w * h).map(|at| rgba[at * 4 + 3] != 0).collect();
    if coloured.iter().all(|has| *has) || coloured.iter().all(|has| !has) {
        return;
    }

    for _ in 0..BLEED_STEPS {
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

pub struct Metered {
    inner: veldsdk::ResourceReader,
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
