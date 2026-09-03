//! Выдержка кодстрима JPEG 2000 под один тайл: главный заголовок, тайл-парты
//! этого тайла не глубже нужного разрешения и EOC — поток, в котором декодеру
//! нечего обходить. Адреса тайл-партов даёт TLM главного заголовка
//! ([`Index`]); где внутри тайл-парта кончается разрешение — PLT его заголовка
//! со счётом пакетов по прецинктам ([`Coding::packets`]): при прогрессии с
//! разрешением во внешнем цикле пакеты грубых разрешений лежат префиксом.
//! Отрезанный хвост дописывается пустыми пакетами — кодстрим остаётся полным,
//! и строгий декодер его принимает. Читатель ([`Reader`]) склеивает куски
//! файла с переписанными здесь байтами и не читает за конец ни одного куска.
//! Что решено и почему — `docs/decisions/0004-jp2-excerpt.md`.

use std::cell::Cell;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

/// Сколько байт тайл-парта берётся первым чтением: заголовок (SOT, PLT, SOD)
/// умещается в сотни байт, а следом лежат пакеты самых грубых разрешений —
/// у гранулы Sentinel-2 три грубейших уровня целиком, и тайл самого грубого
/// стоит одного чтения.
pub const PROBE: u64 = 64 * 1024;

/// Окно чтения куска файла: как у читателя SDK, но не дальше конца куска.
const WINDOW: u64 = 256 * 1024;

/// Тайл-парт по TLM: чей он и где лежит в файле.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub tile: u32,
    pub offset: u64,
    pub len: u64,
}

/// Индекс тайл-партов файла и всё, что нужно собрать выдержку без чтения
/// чужого: байты файла до первого SOT (с коробками JP2, если это контейнер)
/// и параметры кодирования для счёта пакетов.
#[derive(Debug)]
pub struct Index {
    header: Rc<[u8]>,
    parts: Vec<Part>,
    coding: Coding,
}

impl Index {
    /// Индекс из головы файла длиной `len`. Отказ называет, чего не хватило:
    /// TLM, главного заголовка в голове или согласия TLM с длиной кодстрима.
    /// Последнее — не придирка: адреса складываются из длин, и одна лишняя
    /// запись увела бы все тайлы за ней в чужие данные.
    pub fn of(head: &[u8], len: u64) -> Result<Self, String> {
        let (at, end) = super::jp2::codestream_span(head, len).ok_or("кодстрим не найден в голове файла")?;
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
                    // TLM: [Ztlm 1][Stlm 1][записи]; биты 4–5 Stlm — ширина Ttlm
                    // (0, 1, 2 байта), бит 6 — ширина Ptlm (2 или 4). Сегментов
                    // бывает несколько, и записи идут в порядке Ztlm.
                    let (z, stlm) = (*body.first().ok_or("TLM пуст")?, *body.get(1).ok_or("TLM пуст")?);
                    if last_z.is_some_and(|last| z <= last) {
                        return Err("сегменты TLM не по порядку".to_string());
                    }
                    last_z = Some(z);
                    let (tw, pw) = (usize::from((stlm >> 4) & 3), if stlm & 0x40 != 0 { 4 } else { 2 });
                    if (body.len() - 2) % (tw + pw) != 0 {
                        return Err("TLM с обрывком записи".to_string());
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
        let sot = walk.sot().ok_or("главный заголовок не влез в голову файла")?;
        if last_z.is_none() {
            return Err("TLM нет".to_string());
        }
        if coding.comps.iter().any(|comp| comp.precincts.is_empty()) {
            return Err("COD нет".to_string());
        }
        let tiles = u64::from(coding.across()) * u64::from(coding.down());
        let mut offset = at as u64 + sot as u64;
        let mut parts = Vec::with_capacity(entries.len());
        for (ordinal, (tile, len)) in entries.into_iter().enumerate() {
            // Без Ttlm тайл-парт на тайл один, и записи идут по индексам тайлов.
            let tile = tile.unwrap_or(ordinal as u32);
            if u64::from(tile) >= tiles || len < 14 {
                return Err(format!("запись TLM {} не про этот кодстрим", ordinal));
            }
            parts.push(Part { tile, offset, len });
            offset += len;
        }
        if offset + 2 != end {
            return Err(format!("TLM расходится с длиной кодстрима: {} против {}", offset + 2, end));
        }
        Ok(Self { header: Rc::from(&head[..at + sot]), parts, coding })
    }

    /// Тайл-парты тайла в порядке кодстрима.
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

/// Параметры кодирования, от которых зависит число пакетов тайла: сетка
/// холста и тайлов из SIZ, подвыборка, ступени и прецинкты компонентов из COD
/// и COC (ISO 15444-1, B.6–B.12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coding {
    canvas: (u32, u32),
    origin: (u32, u32),
    tile: (u32, u32),
    tile_origin: (u32, u32),
    comps: Vec<Component>,
    layers: u16,
    progression: u8,
    /// SOP или EPH: пустой пакет тогда не один нулевой байт.
    marked: bool,
    /// POC или PPM: порядок пакетов задан не одной прогрессией, либо их
    /// заголовки вынесены из тела.
    changes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Component {
    dx: u8,
    dy: u8,
    /// Показатели ширины и высоты прецинкта по разрешениям, от грубого к
    /// подробному; длина — число разрешений (ступеней + 1).
    precincts: Vec<(u8, u8)>,
}

impl Coding {
    fn from_siz(cs: &[u8]) -> Result<Self, String> {
        // SOC, потом SIZ: [FF51][Lsiz][Rsiz][Xsiz][Ysiz][XOsiz][YOsiz][XTsiz]
        // [YTsiz][XTOsiz][YTOsiz][Csiz][Ssiz XRsiz YRsiz]×Csiz.
        let siz = 2usize;
        if cs.len() < siz + 40 {
            return Err("кодстрим без SIZ".to_string());
        }
        let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
        let count = usize::from(u16::from_be_bytes([cs[siz + 38], cs[siz + 39]]));
        if cs.len() < siz + 40 + 3 * count {
            return Err("SIZ короче своих компонентов".to_string());
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

    /// COD: [Scod][SGcod: прогрессия, слоёв u16, MCT][SPcod: ступеней, кодблок
    /// ×2, стиль, вейвлет, прецинкты по разрешениям при Scod&1].
    fn read_cod(&mut self, body: &[u8]) -> Result<(), String> {
        if body.len() < 10 {
            return Err("COD короче положенного".to_string());
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

    /// COC: [Ccoc 1 или 2][Scoc][SPcoc как у COD] — для одного компонента.
    fn read_coc(&mut self, body: &[u8]) -> Result<(), String> {
        let wide = self.comps.len() > 256;
        let (index, rest) = match (wide, body) {
            (false, [c, rest @ ..]) => (usize::from(*c), rest),
            (true, [hi, lo, rest @ ..]) => (usize::from(u16::from_be_bytes([*hi, *lo])), rest),
            _ => return Err("COC пуст".to_string()),
        };
        if rest.len() < 6 {
            return Err("COC короче положенного".to_string());
        }
        let precincts = precincts_of(rest[0], rest[1], &rest[6..])?;
        self.comps.get_mut(index).ok_or("COC про несуществующий компонент")?.precincts = precincts;
        Ok(())
    }

    fn across(&self) -> u32 {
        span(self.canvas.0, self.tile_origin.0, self.tile.0)
    }

    fn down(&self) -> u32 {
        span(self.canvas.1, self.tile_origin.1, self.tile.1)
    }

    /// Ступеней у самого разложенного компонента: фактор декодера считается
    /// от них.
    pub fn levels(&self) -> u32 {
        self.comps.iter().map(|comp| comp.precincts.len() as u32).max().unwrap_or(0).saturating_sub(1)
    }

    /// Лежат ли пакеты грубых разрешений префиксом тайл-парта: разрешение —
    /// внешний цикл у RLCP и RPCL всегда, у LRCP — при одном слое; POC и PPM
    /// порядок ломают, SOP и EPH меняют вид пустого пакета.
    pub fn prefixed(&self) -> bool {
        !self.changes
            && !self.marked
            && matches!((self.progression, self.layers), (0, 1) | (1, _) | (2, _))
    }

    /// Пакетов тайла по разрешениям не выше `through` (B.6: прецинкты
    /// тайла-компонента на каждом разрешении, помноженные на слои).
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

/// Тайлов вдоль стороны (B.3).
fn span(canvas: u32, origin: u32, step: u32) -> u32 {
    match step {
        0 => 0,
        _ => canvas.saturating_sub(origin).div_ceil(step),
    }
}

/// Показатели прецинктов по разрешениям: свои при бите 0 стиля — по байту на
/// разрешение, младшая тетрада ширина, старшая высота; иначе 2^15 везде.
fn precincts_of(style: u8, levels: u8, tail: &[u8]) -> Result<Vec<(u8, u8)>, String> {
    let resolutions = usize::from(levels) + 1;
    match style & 1 {
        0 => Ok(vec![(15, 15); resolutions]),
        _ => match tail.len() >= resolutions {
            true => Ok(tail[..resolutions].iter().map(|b| (b & 15, b >> 4)).collect()),
            false => Err("прецинктов в COD меньше, чем разрешений".to_string()),
        },
    }
}

/// Кусок выдержки: байты, переписанные здесь, либо кусок файла как он есть.
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

/// Заголовок тайл-парта, разобранный из пробы: где начинаются данные, длины
/// пакетов из PLT и не переопределяет ли он кодирование.
struct Head {
    sod: usize,
    plt: Option<Vec<u64>>,
    overrides: bool,
}

/// Заголовок из пробы; `None` — до SOD проба не дотянулась.
fn head_of(probe: &[u8]) -> Option<Head> {
    let mut walk = 12usize;
    let mut plt = Vec::new();
    // Zplt последнего сегмента PLT: длины склеиваются по порядку сегментов,
    // и сегмент не на своём месте делает PLT нечитаемым — как у TLM.
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

/// Длины пакетов PLT: по семь бит на байт, старший бит — «продолжение».
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

/// Выдержка тайла `tile` на факторе `factor` из проб его тайл-партов —
/// по `probe.len().min(part.len)` байт с начала каждого, в порядке
/// [`Index::parts_of`]. Куски: главный заголовок, тайл-парты (целиком либо
/// до нужного разрешения с дописанными пустыми пакетами), EOC.
///
/// Обрезка возможна, когда прогрессия кладёт разрешения префиксом, у каждого
/// тайл-парта есть PLT, заголовок тайла кодирование не переопределяет и число
/// записей PLT сходится со счётом пакетов; иначе тайл-парты идут целиком —
/// это гранулярность тайла, и она честная.
pub fn assemble(index: &Index, tile: u32, factor: u32, probes: Vec<Vec<u8>>) -> Result<Vec<Segment>, String> {
    let parts = index.parts_of(tile);
    if parts.is_empty() {
        return Err(format!("тайла {} нет в TLM", tile));
    }
    if probes.len() != parts.len() {
        return Err("проб не по числу тайл-партов".to_string());
    }
    let mut heads = Vec::with_capacity(parts.len());
    for (ordinal, (part, probe)) in parts.iter().zip(&probes).enumerate() {
        if probe.len() as u64 != part.len.min(PROBE) || probe.len() < 12 {
            return Err(format!("проба тайл-парта {} не той длины", ordinal));
        }
        // SOT: [FF90][Lsot 10][Isot u16][Psot u32][TPsot][TNsot].
        let isot = u32::from(u16::from_be_bytes([probe[4], probe[5]]));
        let psot = u64::from(u32::from_be_bytes(probe[6..10].try_into().unwrap()));
        if probe[..4] != [0xFF, 0x90, 0x00, 0x0A] || isot != tile || (psot != 0 && psot != part.len) || usize::from(probe[10]) != ordinal {
            return Err(format!("тайл-парт {} тайла {} не там, где обещал TLM", ordinal, tile));
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

    // Где кончается нужное: тайл-парт, на котором набирается `wanted`
    // пакетов, и сколько его пакетов в это число входит. Кончиться нужное
    // может и ровно на границе тайл-парта — тогда он целый, а следующих нет.
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
                let head = heads[ordinal].as_ref().expect("обрезка решена по заголовкам");
                let plt = head.plt.as_ref().expect("обрезка решена по PLT");
                (head.sod as u64 + plt[..packets].iter().sum::<u64>(), total - wanted)
            }
            _ => (part.len, 0),
        };
        // Копия заголовка своя, и переписать в ней можно что угодно: длину
        // тайл-парта — вместе с пустыми пакетами, число тайл-партов —
        // сколько их осталось.
        if keep < part.len || pad > 0 {
            probe[6..10].copy_from_slice(&((keep + pad) as u32).to_be_bytes());
        }
        // Число тайл-партов шире байта стандарт пишет нулём — «не указано».
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

/// Чтение куска файла: смещение и длина — байты, ровно столько.
pub type Fetch = Box<dyn FnMut(u64, u64) -> io::Result<Vec<u8>>>;

/// Читатель выдержки: куски подряд, как один поток, с произвольным seek.
/// Кусок файла читается окнами не дальше своего конца; счётчик `bytes` —
/// сумма прочитанного из файла.
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
    /// Над ресурсом модуля: куски файла читаются у хоста.
    pub fn over(resource: u64, segments: Vec<Segment>, bytes: Rc<Cell<u64>>) -> Self {
        Self::over_fetch(
            segments,
            bytes,
            Box::new(move |offset, size| {
                veldsdk::abi::resource_read(resource, offset, size).map_err(|e| {
                    io::Error::other(format!("resource {}: чтение {} байт со смещения {}: {}", resource, size, offset, e))
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
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "пустое чтение куска выдержки"));
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
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek до начала выдержки"));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Кодирование гранулы Sentinel-2: три компонента без подвыборки, четыре
    /// ступени, прецинкты 256² на всех разрешениях.
    fn granule() -> Coding {
        let comp = Component { dx: 1, dy: 1, precincts: vec![(8, 8); 5] };
        Coding {
            canvas: (10980, 10980),
            origin: (0, 0),
            tile: (1024, 1024),
            tile_origin: (0, 0),
            comps: vec![comp.clone(), comp.clone(), comp],
            layers: 1,
            progression: 0,
            marked: false,
            changes: false,
        }
    }

    /// Счёт пакетов сходится с PLT настоящей гранулы (замер 3 сентября 2026):
    /// у полного тайла по разрешениям 3, 3, 3, 12, 48 пакетов, у краевого
    /// 740² — 3, 3, 3, 12, 27.
    #[test]
    fn packets_are_counted_as_the_granule_writes_them() {
        let coding = granule();
        assert_eq!((coding.across(), coding.down()), (11, 11));
        let by_resolution = |tile: u32| -> Vec<u64> {
            (0..5).map(|r| coding.packets(tile, r) - if r == 0 { 0 } else { coding.packets(tile, r - 1) }).collect()
        };
        assert_eq!(by_resolution(0), [3, 3, 3, 12, 48]);
        assert_eq!(by_resolution(120), [3, 3, 3, 12, 27]);
        assert_eq!(coding.packets(0, u32::MAX), 69);
        assert_eq!(coding.levels(), 4);
        assert!(coding.prefixed());

        // Прецинкты по умолчанию — один на разрешение, а два слоя удваивают.
        let mut plain = granule();
        for comp in &mut plain.comps {
            comp.precincts = vec![(15, 15); 5];
        }
        plain.layers = 2;
        assert_eq!(plain.packets(0, u32::MAX), 30);
        assert!(!plain.prefixed(), "LRCP с двумя слоями не кладёт разрешения префиксом");
        plain.progression = 1;
        assert!(plain.prefixed(), "у RLCP разрешение — внешний цикл при любых слоях");
    }

    /// Главный заголовок для индекса: SIZ на один компонент, COD, TLM с
    /// данными записями — до первого SOT, который приносит тайл-парт.
    fn main_header(canvas: u32, tile: u32, levels: u8, tlm: &[(u16, u32)]) -> Vec<u8> {
        main_header_with(canvas, tile, levels, tlm, 0, &[])
    }

    /// Тот же заголовок со стилем кодирования `scod` в COD и лишним сегментом
    /// `extra` (маркер, длина, тело) после него.
    fn main_header_with(canvas: u32, tile: u32, levels: u8, tlm: &[(u16, u32)], scod: u8, extra: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, 0x4F, 0xFF, 0x51];
        let mut siz = 0u16.to_be_bytes().to_vec();
        for value in [canvas, canvas, 0, 0, tile, tile, 0, 0] {
            siz.extend_from_slice(&value.to_be_bytes());
        }
        siz.extend_from_slice(&1u16.to_be_bytes());
        siz.extend_from_slice(&[7, 1, 1]);
        out.extend_from_slice(&((siz.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&siz);
        let cod = [scod, 0, 0, 1, 0, levels, 4, 4, 0, 1];
        out.extend_from_slice(&[0xFF, 0x52]);
        out.extend_from_slice(&((cod.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&cod);
        out.extend_from_slice(extra);
        let mut body = vec![0u8, 0x60]; // Ztlm 0, Stlm: Ttlm u16, Ptlm u32
        for (tile, len) in tlm {
            body.extend_from_slice(&tile.to_be_bytes());
            body.extend_from_slice(&len.to_be_bytes());
        }
        out.extend_from_slice(&[0xFF, 0x55]);
        out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Длина пакета по правилу PLT: семь бит на байт, старший — «дальше».
    fn varint(value: u32) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut rest = value;
        loop {
            out.insert(0, (rest & 0x7F) as u8 | if out.is_empty() { 0 } else { 0x80 });
            rest >>= 7;
            if rest == 0 {
                return out;
            }
        }
    }

    /// Тайл-парт: SOT, PLT с данными длинами, SOD, данные.
    fn tile_part(tile: u16, ordinal: u8, count: u8, packets: &[u32]) -> Vec<u8> {
        tile_part_with(tile, ordinal, count, packets, &[])
    }

    /// Тот же тайл-парт с лишним сегментом `extra` (маркер, длина, тело)
    /// в заголовке перед PLT.
    fn tile_part_with(tile: u16, ordinal: u8, count: u8, packets: &[u32], extra: &[u8]) -> Vec<u8> {
        let mut plt = vec![0u8];
        for n in packets {
            plt.extend_from_slice(&varint(*n));
        }
        let data: Vec<u8> = packets.iter().flat_map(|n| (1..=*n).map(|i| i as u8).collect::<Vec<u8>>()).collect();
        let len = 12 + extra.len() + 4 + plt.len() + 2 + data.len();
        let mut out = vec![0xFF, 0x90, 0x00, 0x0A];
        out.extend_from_slice(&tile.to_be_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(&[ordinal, count]);
        out.extend_from_slice(extra);
        out.extend_from_slice(&[0xFF, 0x58]);
        out.extend_from_slice(&((plt.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&plt);
        out.extend_from_slice(&[0xFF, 0x93]);
        out.extend_from_slice(&data);
        out
    }

    /// Файл из главного заголовка и тайл-партов; TLM в заголовке — по ним.
    fn file(canvas: u32, tile: u32, levels: u8, parts: &[(u16, Vec<u8>)]) -> (Vec<u8>, Index) {
        let tlm: Vec<(u16, u32)> = parts.iter().map(|(tile, bytes)| (*tile, bytes.len() as u32)).collect();
        let mut bytes = main_header(canvas, tile, levels, &tlm);
        for (_, part) in parts {
            bytes.extend_from_slice(part);
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        let index = Index::of(&bytes, bytes.len() as u64).expect("индекс строится");
        (bytes, index)
    }

    /// Индекс складывает адреса из длин TLM от первого SOT и требует, чтобы
    /// они сошлись с концом кодстрима; без TLM индекса нет.
    #[test]
    fn the_index_follows_tlm_and_checks_the_sum() {
        let parts = [(0u16, tile_part(0, 0, 1, &[3, 4, 5])), (1, tile_part(1, 0, 1, &[6, 7, 8]))];
        assert_eq!(varint(70_000), [0x84, 0xA2, 0x70], "три семёрки бит со старшими «дальше»");
        let (bytes, index) = file(128, 64, 2, &parts);
        let sot = bytes.windows(2).position(|pair| pair == [0xFF, 0x90]).unwrap() as u64;
        assert_eq!(index.parts(), 2);
        assert_eq!(index.parts_of(1), [Part { tile: 1, offset: sot + parts[0].1.len() as u64, len: parts[1].1.len() as u64 }]);
        assert!(index.parts_of(2).is_empty());

        let mut lying = bytes.clone();
        lying.extend_from_slice(&[0; 7]);
        let why = Index::of(&lying, lying.len() as u64).expect_err("сумма не сошлась");
        assert!(why.contains("расходится"), "{why}");
        let short = &bytes[..sot as usize - 3];
        assert!(Index::of(short, bytes.len() as u64).expect_err("SOT не в голове").contains("не влез"));
    }

    /// Выдержка режет тайл-парт после пакетов нужного разрешения, дописывает
    /// пустые пакеты за отрезанные, переписывает Psot и TNsot — и не трогает
    /// ничего, когда нужен весь тайл.
    #[test]
    fn an_excerpt_ends_after_the_level_and_pads_the_rest() {
        // Один компонент, две ступени: по пакету на разрешение, три пакета
        // в тайле; у тайла 1 они разложены по двум тайл-партам.
        let parts = [
            (0u16, tile_part(0, 0, 1, &[3, 4, 5])),
            (1, tile_part(1, 0, 2, &[6, 7])),
            (1, tile_part(1, 1, 2, &[8])),
        ];
        let (bytes, index) = file(128, 64, 2, &parts);
        let probes = |tile: u32| -> Vec<Vec<u8>> {
            index.parts_of(tile).iter().map(|part| bytes[part.offset as usize..(part.offset + part.len) as usize].to_vec()).collect()
        };
        let header = &bytes[..bytes.windows(2).position(|pair| pair == [0xFF, 0x90]).unwrap()];

        // Фактор 1: два грубых пакета — первый тайл-парт целиком, второго
        // нет, вместо его пакета пустой; TNsot стал единицей.
        let cut = assemble(&index, 1, 1, probes(1)).expect("собирается");
        let Segment::Bytes(part) = &cut[1] else { panic!("тайл-парт переписан") };
        let expect_len = parts[1].1.len() as u32 + 1;
        assert_eq!(&part[6..10], &expect_len.to_be_bytes(), "Psot — свои байты плюс пустой пакет");
        assert_eq!(part[11], 1, "TNsot — сколько тайл-партов осталось");
        assert_eq!(&part[12..], &parts[1].1[12..], "данные первого тайл-парта целы");
        assert_eq!(cut[2], Segment::Bytes(Rc::from(&[0u8][..])), "пустой пакет — нулевой байт");
        assert_eq!(cut[3], Segment::Bytes(Rc::from(&[0xFFu8, 0xD9][..])));
        assert_eq!(cut.len(), 4);
        assert_eq!(cut[0], Segment::Bytes(Rc::from(header)));

        // Фактор 2: один пакет из первого тайл-парта, два пустых.
        let cut = assemble(&index, 1, 2, probes(1)).expect("собирается");
        let Segment::Bytes(part) = &cut[1] else { panic!() };
        let sod = 12 + 4 + 3 + 2;
        assert_eq!(part.len(), sod + 6, "заголовок и первый пакет");
        assert_eq!(&part[6..10], &((sod + 6 + 2) as u32).to_be_bytes());
        assert_eq!(cut[2], Segment::Bytes(Rc::from(&[0u8, 0][..])));

        // Фактор 0: всё нужно — оба тайл-парта как есть, без дописок.
        let whole = assemble(&index, 1, 0, probes(1)).expect("собирается");
        assert_eq!(whole.len(), 4);
        for (segment, (_, part)) in whole[1..3].iter().zip(&parts[1..]) {
            assert_eq!(segment, &Segment::Bytes(Rc::from(part.as_slice())));
        }

        // Проба не с того места — отказ по имени, а не тихая выдержка.
        let mut wrong = probes(1);
        wrong[0][5] = 0;
        assert!(assemble(&index, 1, 1, wrong).expect_err("TLM соврал").contains("не там"));
    }

    /// Хвост нужного, не попавший в пробу, идёт куском файла — ровно до
    /// конца нужного, а не до конца тайл-парта; читатель отдаёт выдержку
    /// байт в байт как переписанный поток.
    #[test]
    fn what_the_probe_missed_comes_from_the_file() {
        let parts = [(0u16, tile_part(0, 0, 1, &[70_000, 10, 10]))];
        let (bytes, index) = file(64, 64, 2, &parts);
        let part = index.parts_of(0)[0];
        assert!(part.len > PROBE, "тайл-парт длиннее пробы");
        let sod = 12 + 4 + (1 + 3 + 1 + 1) + 2;
        let probe = bytes[part.offset as usize..(part.offset + PROBE) as usize].to_vec();

        let cut = assemble(&index, 0, 1, vec![probe]).expect("собирается");
        let keep = sod + 70_000 + 10;
        assert_eq!(cut.len(), 5);
        assert_eq!(cut[2], Segment::File { offset: part.offset + PROBE, len: keep - PROBE });
        assert_eq!(cut[3], Segment::Bytes(Rc::from(&[0u8][..])));

        let source = bytes.clone();
        let fetch: Fetch = Box::new(move |offset, size| Ok(source[offset as usize..(offset + size) as usize].to_vec()));
        let mut reader = Reader::over_fetch(cut, Rc::new(Cell::new(0)), fetch);
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        let mut expected = bytes[..part.offset as usize].to_vec();
        let mut head = bytes[part.offset as usize..(part.offset + keep) as usize].to_vec();
        head[6..10].copy_from_slice(&((keep + 1) as u32).to_be_bytes());
        expected.extend_from_slice(&head);
        expected.extend_from_slice(&[0, 0xFF, 0xD9]);
        assert_eq!(got, expected);
    }

    /// Читатель отдаёт куски подряд, читает файл окнами не за конец куска и
    /// принимает seek в любую сторону.
    #[test]
    fn the_reader_splices_segments_and_never_reads_past_a_piece() {
        let disk: Vec<u8> = (0..=255u8).cycle().take(1_000_000).collect();
        let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = asked.clone();
        let from = disk.clone();
        let fetch: Fetch = Box::new(move |offset, size| {
            log.borrow_mut().push((offset, size));
            Ok(from[offset as usize..(offset + size) as usize].to_vec())
        });
        let segments = vec![
            Segment::Bytes(Rc::from(&b"head"[..])),
            Segment::File { offset: 1000, len: 600_000 },
            Segment::Bytes(Rc::from(&[0u8, 0][..])),
            Segment::File { offset: 5, len: 3 },
        ];
        let bytes = Rc::new(Cell::new(0));
        let mut reader = Reader::over_fetch(segments, bytes.clone(), fetch);
        assert_eq!(reader.len(), 4 + 600_000 + 2 + 3);
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        let mut expected = b"head".to_vec();
        expected.extend_from_slice(&disk[1000..601_000]);
        expected.extend_from_slice(&[0, 0]);
        expected.extend_from_slice(&disk[5..8]);
        assert_eq!(got, expected);
        assert_eq!(
            *asked.borrow(),
            vec![(1000, 262_144), (263_144, 262_144), (525_288, 75_712), (5, 3)],
            "окна не длиннее WINDOW и не дальше конца куска"
        );
        assert_eq!(bytes.get(), 600_003);

        reader.seek(SeekFrom::Start(2)).unwrap();
        let mut two = [0u8; 4];
        reader.read_exact(&mut two).unwrap();
        assert_eq!(&two, &[b'a', b'd', disk[1000], disk[1001]], "стык кусков читается сплошь");
        reader.seek(SeekFrom::End(-3)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &disk[5..8]);
        reader.seek(SeekFrom::Start(10)).unwrap();
        assert!(reader.seek(SeekFrom::Current(-100)).is_err(), "до начала выдержки seek нет");
    }

    /// Заголовок тайла, переопределяющий кодирование (COD, COC, POC, PPT),
    /// отменяет обрезку: счёт пакетов ведётся по главному заголовку, и с
    /// чужими прецинктами он соврёт. Тайл-парт идёт целиком, без дописок.
    #[test]
    fn a_tile_header_that_overrides_coding_keeps_the_part_whole() {
        let cod = [0xFF, 0x52, 0x00, 0x0C, 0, 0, 0, 1, 0, 2, 4, 4, 0, 1];
        let parts = [(0u16, tile_part_with(0, 0, 1, &[3, 4, 5], &cod))];
        let (bytes, index) = file(64, 64, 2, &parts);
        let part = index.parts_of(0)[0];
        let probe = bytes[part.offset as usize..(part.offset + part.len) as usize].to_vec();
        let whole = assemble(&index, 0, 2, vec![probe]).expect("собирается");
        assert_eq!(whole.len(), 3, "заголовок, тайл-парт целиком, EOC");
        assert_eq!(whole[1], Segment::Bytes(Rc::from(parts[0].1.as_slice())));
    }

    /// Порядок пакетов, в котором разрешение не внешний цикл, префикса не
    /// даёт: POC и PPM в главном заголовке, SOP и EPH в стиле кодирования —
    /// каждый порознь запрещает обрезку, и берётся это из самого заголовка.
    #[test]
    fn changes_and_markers_forbid_the_prefix() {
        let part = tile_part(0, 0, 1, &[3, 4, 5]);
        let index_with = |scod: u8, extra: &[u8]| {
            let mut bytes = main_header_with(64, 64, 2, &[(0, part.len() as u32)], scod, extra);
            bytes.extend_from_slice(&part);
            bytes.extend_from_slice(&[0xFF, 0xD9]);
            Index::of(&bytes, bytes.len() as u64).expect("индекс строится")
        };
        assert!(index_with(0, &[]).coding().prefixed(), "LRCP с одним слоем — префикс");
        let poc = [0xFF, 0x5F, 0x00, 0x09, 0, 0, 0, 0, 0, 0, 0];
        assert!(!index_with(0, &poc).coding().prefixed(), "POC ломает порядок");
        assert!(!index_with(0, &[0xFF, 0x60, 0x00, 0x03, 0]).coding().prefixed(), "PPM выносит заголовки пакетов");
        assert!(!index_with(0x02, &[]).coding().prefixed(), "SOP меняет вид пустого пакета");
        assert!(!index_with(0x04, &[]).coding().prefixed(), "EPH тоже");
    }

    /// Индекс отказывает по имени, а не молчит: сегменты TLM не по порядку,
    /// запись про тайл, которого нет, обрывок записи.
    #[test]
    fn a_broken_tlm_is_refused_by_name() {
        let parts = [(0u16, tile_part(0, 0, 1, &[3, 4, 5]))];
        let (bytes, _) = file(64, 64, 2, &parts);
        let tlm_at = bytes.windows(2).position(|pair| pair == [0xFF, 0x55]).unwrap();

        // Второй TLM с Ztlm 0 после первого с Ztlm 0 — не по порядку.
        let mut twice = bytes.clone();
        twice.splice(tlm_at..tlm_at, [0xFF, 0x55, 0x00, 0x04, 0x00, 0x60]);
        assert!(Index::of(&twice, twice.len() as u64).expect_err("порядок").contains("не по порядку"));

        // Тайл 9 в кодстриме на один тайл.
        let mut foreign = bytes.clone();
        foreign[tlm_at + 6..tlm_at + 8].copy_from_slice(&9u16.to_be_bytes());
        assert!(Index::of(&foreign, foreign.len() as u64).expect_err("чужой тайл").contains("не про этот"));

        // Лишний байт в теле TLM — ни одна ширина записи его не делит.
        let mut torn = bytes.clone();
        torn.splice(tlm_at + 6..tlm_at + 6, [0u8]);
        torn[tlm_at + 3] += 1;
        assert!(Index::of(&torn, torn.len() as u64).expect_err("обрывок").contains("обрывком"));
    }

    /// PLT не по порядку Zplt — длин нет, и тайл-парт идёт целиком.
    #[test]
    fn plt_segments_out_of_order_give_no_lengths() {
        let mut part = tile_part(0, 0, 1, &[3, 4, 5]);
        // Второй PLT с Zplt 0 перед SOD — тем же номером, что и первый.
        let sod = part.windows(2).position(|pair| pair == [0xFF, 0x93]).unwrap();
        part.splice(sod..sod, [0xFF, 0x58, 0x00, 0x04, 0x00, 0x01]);
        let head = head_of(&part).expect("до SOD дошли");
        assert!(head.plt.is_none(), "склеивать не по порядку нельзя");
        let ordered = head_of(&tile_part(0, 0, 1, &[3, 4, 5])).unwrap();
        assert_eq!(ordered.plt, Some(vec![3, 4, 5]));
    }
}
