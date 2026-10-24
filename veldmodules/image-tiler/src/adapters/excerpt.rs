use std::cell::Cell;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

pub const PROBE: u64 = 64 * 1024;

const WINDOW: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub tile: u32,
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug)]
pub struct Index {
    header: Rc<[u8]>,
    parts: Vec<Part>,
    coding: Coding,
}

impl Index {
    pub fn of(head: &[u8], len: u64) -> Result<Self, String> {
        let (at, end) = super::jp2::codestream_span(head, len).ok_or("codestream not found in the file head")?;
        let cs = &head[at..];
        let mut coding = Coding::from_siz(cs)?;
        let mut entries: Vec<(Option<u32>, u64)> = Vec::new();
        let mut last_z = None;
        let mut walk = super::jp2::segments(cs);
        while let Some((marker, body)) = walk.next()? {
            match marker {
                0x52 => coding.read_cod(body)?,
                0x53 => coding.read_coc(body)?,
                0x5F | 0x60 => coding.changes = true,
                0x55 => {
                    let (z, stlm) = (*body.first().ok_or("TLM is empty")?, *body.get(1).ok_or("TLM is empty")?);
                    if last_z.is_some_and(|last| z <= last) {
                        return Err("TLM segments out of order".to_string());
                    }
                    last_z = Some(z);
                    let (tw, pw) = (usize::from((stlm >> 4) & 3), if stlm & 0x40 != 0 { 4 } else { 2 });
                    if (body.len() - 2) % (tw + pw) != 0 {
                        return Err("TLM with a torn entry".to_string());
                    }
                    for entry in body[2..].chunks_exact(tw + pw) {
                        let tile = match tw {
                            0 => None,
                            1 => Some(u32::from(entry[0])),
                            _ => Some(u32::from(u16::from_be_bytes([entry[0], entry[1]]))),
                        };
                        let len = match pw {
                            4 => u64::from(u32::from_be_bytes(entry[tw..tw + 4].try_into().unwrap())),
                            _ => u64::from(u16::from_be_bytes([entry[tw], entry[tw + 1]])),
                        };
                        entries.push((tile, len));
                    }
                }
                _ => {}
            }
        }
        let sot = walk.sot().ok_or("main header does not fit in the file head")?;
        if last_z.is_none() {
            return Err("no TLM".to_string());
        }
        if coding.comps.iter().any(|comp| comp.precincts.is_empty()) {
            return Err("no COD".to_string());
        }
        let tiles = u64::from(coding.across()) * u64::from(coding.down());
        let mut offset = at as u64 + sot as u64;
        let mut parts = Vec::with_capacity(entries.len());
        for (ordinal, (tile, len)) in entries.into_iter().enumerate() {
            let tile = tile.unwrap_or(ordinal as u32);
            if u64::from(tile) >= tiles || len < 14 {
                return Err(format!("TLM entry {} is not for this codestream", ordinal));
            }
            parts.push(Part { tile, offset, len });
            offset += len;
        }
        if offset + 2 != end {
            return Err(format!("TLM disagrees with the codestream length: {} versus {}", offset + 2, end));
        }
        Ok(Self { header: Rc::from(&head[..at + sot]), parts, coding })
    }

    pub fn parts_of(&self, tile: u32) -> Vec<Part> {
        self.parts.iter().copied().filter(|part| part.tile == tile).collect()
    }

    pub fn parts(&self) -> usize {
        self.parts.len()
    }

    pub fn coding(&self) -> &Coding {
        &self.coding
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coding {
    canvas: (u32, u32),
    origin: (u32, u32),
    tile: (u32, u32),
    tile_origin: (u32, u32),
    comps: Vec<Component>,
    layers: u16,
    progression: u8,
    marked: bool,
    changes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Component {
    dx: u8,
    dy: u8,
    precincts: Vec<(u8, u8)>,
}

impl Coding {
    fn from_siz(cs: &[u8]) -> Result<Self, String> {
        let siz = 2usize;
        if cs.len() < siz + 40 {
            return Err("codestream without SIZ".to_string());
        }
        let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
        let count = usize::from(u16::from_be_bytes([cs[siz + 38], cs[siz + 39]]));
        if cs.len() < siz + 40 + 3 * count {
            return Err("SIZ shorter than its components".to_string());
        }
        let comps = (0..count)
            .map(|c| Component { dx: cs[siz + 41 + 3 * c], dy: cs[siz + 42 + 3 * c], precincts: Vec::new() })
            .collect();
        Ok(Self {
            canvas: (be32(siz + 6), be32(siz + 10)),
            origin: (be32(siz + 14), be32(siz + 18)),
            tile: (be32(siz + 22), be32(siz + 26)),
            tile_origin: (be32(siz + 30), be32(siz + 34)),
            comps,
            layers: 0,
            progression: 0,
            marked: false,
            changes: false,
        })
    }

    fn read_cod(&mut self, body: &[u8]) -> Result<(), String> {
        if body.len() < 10 {
            return Err("COD shorter than required".to_string());
        }
        self.progression = body[1];
        self.layers = u16::from_be_bytes([body[2], body[3]]);
        self.marked = body[0] & 0x06 != 0;
        let precincts = precincts_of(body[0], body[5], &body[10..])?;
        for comp in &mut self.comps {
            comp.precincts = precincts.clone();
        }
        Ok(())
    }

    fn read_coc(&mut self, body: &[u8]) -> Result<(), String> {
        let wide = self.comps.len() > 256;
        let (index, rest) = match (wide, body) {
            (false, [c, rest @ ..]) => (usize::from(*c), rest),
            (true, [hi, lo, rest @ ..]) => (usize::from(u16::from_be_bytes([*hi, *lo])), rest),
            _ => return Err("COC is empty".to_string()),
        };
        if rest.len() < 6 {
            return Err("COC shorter than required".to_string());
        }
        let precincts = precincts_of(rest[0], rest[1], &rest[6..])?;
        self.comps.get_mut(index).ok_or("COC for a nonexistent component")?.precincts = precincts;
        Ok(())
    }

    fn across(&self) -> u32 {
        span(self.canvas.0, self.tile_origin.0, self.tile.0)
    }

    fn down(&self) -> u32 {
        span(self.canvas.1, self.tile_origin.1, self.tile.1)
    }

    pub fn levels(&self) -> u32 {
        self.comps.iter().map(|comp| comp.precincts.len() as u32).max().unwrap_or(0).saturating_sub(1)
    }

    pub fn prefixed(&self) -> bool {
        !self.changes
            && !self.marked
            && matches!((self.progression, self.layers), (0, 1) | (1, _) | (2, _))
    }

    pub fn packets(&self, tile: u32, through: u32) -> u64 {
        let across = u64::from(self.across().max(1));
        let (p, q) = (u64::from(tile) % across, u64::from(tile) / across);
        let bound = |origin: u32, step: u32, canvas_origin: u32, canvas: u32, k: u64| {
            let from = (u64::from(origin) + k * u64::from(step)).max(u64::from(canvas_origin));
            let to = (u64::from(origin) + (k + 1) * u64::from(step)).min(u64::from(canvas));
            (from, to)
        };
        let (tx0, tx1) = bound(self.tile_origin.0, self.tile.0, self.origin.0, self.canvas.0, p);
        let (ty0, ty1) = bound(self.tile_origin.1, self.tile.1, self.origin.1, self.canvas.1, q);
        let mut total = 0u64;
        for comp in &self.comps {
            let (dx, dy) = (u64::from(comp.dx.max(1)), u64::from(comp.dy.max(1)));
            let (tcx0, tcx1) = (tx0.div_ceil(dx), tx1.div_ceil(dx));
            let (tcy0, tcy1) = (ty0.div_ceil(dy), ty1.div_ceil(dy));
            let levels = (comp.precincts.len() as u32).saturating_sub(1);
            for (r, &(ppx, ppy)) in comp.precincts.iter().enumerate().take_while(|(r, _)| *r as u32 <= through) {
                let scale = 1u64 << (levels - r as u32);
                let (trx0, trx1) = (tcx0.div_ceil(scale), tcx1.div_ceil(scale));
                let (try0, try1) = (tcy0.div_ceil(scale), tcy1.div_ceil(scale));
                let wide = match trx1 > trx0 {
                    true => trx1.div_ceil(1 << ppx) - (trx0 >> ppx),
                    false => 0,
                };
                let high = match try1 > try0 {
                    true => try1.div_ceil(1 << ppy) - (try0 >> ppy),
                    false => 0,
                };
                total += wide * high * u64::from(self.layers);
            }
        }
        total
    }
}

fn span(canvas: u32, origin: u32, step: u32) -> u32 {
    match step {
        0 => 0,
        _ => canvas.saturating_sub(origin).div_ceil(step),
    }
}

fn precincts_of(style: u8, levels: u8, tail: &[u8]) -> Result<Vec<(u8, u8)>, String> {
    let resolutions = usize::from(levels) + 1;
    match style & 1 {
        0 => Ok(vec![(15, 15); resolutions]),
        _ => match tail.len() >= resolutions {
            true => Ok(tail[..resolutions].iter().map(|b| (b & 15, b >> 4)).collect()),
            false => Err("fewer precincts in COD than resolutions".to_string()),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Bytes(Rc<[u8]>),
    File { offset: u64, len: u64 },
}

impl Segment {
    fn len(&self) -> u64 {
        match self {
            Segment::Bytes(bytes) => bytes.len() as u64,
            Segment::File { len, .. } => *len,
        }
    }
}

struct Head {
    sod: usize,
    plt: Option<Vec<u64>>,
    overrides: bool,
}

fn head_of(probe: &[u8]) -> Option<Head> {
    let mut walk = 12usize;
    let mut plt = Vec::new();
    let mut last_z: Option<u8> = None;
    let mut ordered = true;
    let mut overrides = false;
    while walk + 4 <= probe.len() && probe[walk] == 0xFF {
        let marker = probe[walk + 1];
        if marker == 0x93 {
            let plt = (last_z.is_some() && ordered).then(|| varints(&plt));
            return Some(Head { sod: walk + 2, plt, overrides });
        }
        let length = usize::from(u16::from_be_bytes([probe[walk + 2], probe[walk + 3]]));
        if length < 2 || walk + 2 + length > probe.len() {
            return None;
        }
        let body = &probe[walk + 4..walk + 2 + length];
        match marker {
            0x58 => {
                let z = body.first().copied().unwrap_or(0);
                ordered &= last_z.is_none_or(|last| z > last);
                last_z = Some(z);
                plt.extend_from_slice(body.get(1..).unwrap_or_default());
            }
            0x52 | 0x53 | 0x5F | 0x61 => overrides = true,
            _ => {}
        }
        walk += 2 + length;
    }
    None
}

fn varints(bytes: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut value = 0u64;
    for &b in bytes {
        value = (value << 7) | u64::from(b & 0x7F);
        if b & 0x80 == 0 {
            out.push(value);
            value = 0;
        }
    }
    out
}

pub fn assemble(index: &Index, tile: u32, factor: u32, probes: Vec<Vec<u8>>) -> Result<Vec<Segment>, String> {
    let parts = index.parts_of(tile);
    if parts.is_empty() {
        return Err(format!("tile {} is not in TLM", tile));
    }
    if probes.len() != parts.len() {
        return Err("probes do not match the number of tile parts".to_string());
    }
    let mut heads = Vec::with_capacity(parts.len());
    for (ordinal, (part, probe)) in parts.iter().zip(&probes).enumerate() {
        if probe.len() as u64 != part.len.min(PROBE) || probe.len() < 12 {
            return Err(format!("probe of tile part {} has the wrong length", ordinal));
        }
        let isot = u32::from(u16::from_be_bytes([probe[4], probe[5]]));
        let psot = u64::from(u32::from_be_bytes(probe[6..10].try_into().unwrap()));
        if probe[..4] != [0xFF, 0x90, 0x00, 0x0A] || isot != tile || (psot != 0 && psot != part.len) || usize::from(probe[10]) != ordinal {
            return Err(format!("tile part {} of tile {} is not where TLM promised", ordinal, tile));
        }
        heads.push(head_of(probe));
    }

    let coding = index.coding();
    let total = coding.packets(tile, u32::MAX);
    let wanted = coding.packets(tile, coding.levels().saturating_sub(factor));
    let listed: Option<u64> = heads.iter().try_fold(0u64, |sum, head| {
        head.as_ref().and_then(|head| head.plt.as_ref()).map(|plt| sum + plt.len() as u64)
    });
    let cut = coding.prefixed()
        && wanted > 0
        && wanted < total
        && listed == Some(total)
        && heads.iter().all(|head| head.as_ref().is_some_and(|head| !head.overrides))
        && parts.len() <= 255;

    let ending = cut.then(|| {
        let mut before = 0u64;
        for (ordinal, head) in heads.iter().enumerate() {
            let listed = head.as_ref().and_then(|head| head.plt.as_ref()).map_or(0, |plt| plt.len() as u64);
            if before + listed >= wanted {
                return (ordinal, (wanted - before) as usize);
            }
            before += listed;
        }
        (parts.len() - 1, 0)
    });
    let kept = ending.map_or(parts.len(), |(ordinal, _)| ordinal + 1);

    let mut segments = vec![Segment::Bytes(index.header.clone())];
    for (ordinal, (part, mut probe)) in parts.iter().zip(probes).take(kept).enumerate() {
        let (keep, pad) = match ending {
            Some((at, packets)) if at == ordinal => {
                let head = heads[ordinal].as_ref().expect("cut is decided by the headers");
                let plt = head.plt.as_ref().expect("cut is decided by the PLT");
                (head.sod as u64 + plt[..packets].iter().sum::<u64>(), total - wanted)
            }
            _ => (part.len, 0),
        };
        if keep < part.len || pad > 0 {
            probe[6..10].copy_from_slice(&((keep + pad) as u32).to_be_bytes());
        }
        probe[11] = u8::try_from(kept).unwrap_or(0);
        let probed = (probe.len() as u64).min(keep);
        probe.truncate(probed as usize);
        segments.push(Segment::Bytes(Rc::from(probe)));
        if keep > probed {
            segments.push(Segment::File { offset: part.offset + probed, len: keep - probed });
        }
        if pad > 0 {
            segments.push(Segment::Bytes(Rc::from(vec![0u8; pad as usize])));
        }
    }
    segments.push(Segment::Bytes(Rc::from(&[0xFFu8, 0xD9][..])));
    Ok(segments)
}

pub type Fetch = Box<dyn FnMut(u64, u64) -> io::Result<Vec<u8>>>;

pub struct Reader {
    segments: Vec<Segment>,
    starts: Vec<u64>,
    len: u64,
    pos: u64,
    window: Vec<u8>,
    window_at: u64,
    bytes: Rc<Cell<u64>>,
    fetch: Fetch,
}

impl Reader {
    pub fn over(resource: u64, segments: Vec<Segment>, bytes: Rc<Cell<u64>>) -> Self {
        Self::over_fetch(
            segments,
            bytes,
            Box::new(move |offset, size| {
                veldsdk::abi::resource_read(resource, offset, size).map_err(|e| {
                    io::Error::other(format!("resource {}: reading {} bytes at offset {}: {}", resource, size, offset, e))
                })
            }),
        )
    }

    pub fn over_fetch(segments: Vec<Segment>, bytes: Rc<Cell<u64>>, fetch: Fetch) -> Self {
        let mut starts = Vec::with_capacity(segments.len());
        let mut len = 0u64;
        for segment in &segments {
            starts.push(len);
            len += segment.len();
        }
        Self { segments, starts, len, pos: 0, window: Vec::new(), window_at: 0, bytes, fetch }
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let which = self.starts.partition_point(|start| *start <= self.pos) - 1;
        let within = self.pos - self.starts[which];
        let n = match &self.segments[which] {
            Segment::Bytes(bytes) => {
                let from = within as usize;
                let n = buf.len().min(bytes.len() - from);
                buf[..n].copy_from_slice(&bytes[from..from + n]);
                n
            }
            Segment::File { offset, len } => {
                let covered = self.pos >= self.window_at && self.pos < self.window_at + self.window.len() as u64;
                if !covered {
                    let size = WINDOW.min(len - within);
                    let mut data = (self.fetch)(offset + within, size)?;
                    data.truncate(size as usize);
                    if data.is_empty() {
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "empty read of an excerpt piece"));
                    }
                    self.bytes.set(self.bytes.get() + data.len() as u64);
                    self.window = data;
                    self.window_at = self.pos;
                }
                let from = (self.pos - self.window_at) as usize;
                let n = buf.len().min(self.window.len() - from);
                buf[..n].copy_from_slice(&self.window[from..from + n]);
                n
            }
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Reader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.len as i64 + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before the start of the excerpt"));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}
