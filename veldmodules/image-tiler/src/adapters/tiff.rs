//! TIFF: единственный формат с дешёвым произвольным доступом — и он же
//! умеет быть самым дорогим, когда раскладка полосная и без обзоров.
//!
//! Прямой путь (тайловый файл с уменьшенными копиями, COG): тайл уровня — это
//! несколько чанков ближайшей копии, прочитанных и ужатых точно в сетку
//! уровня. Копии не обязаны быть степенями двойки, поэтому масштаб дробный и
//! проходит через общий ресемплер.
//!
//! Последовательный путь (полосы или тайлы без копий): один проход по чанкам
//! сверху вниз, ряд чанков собирается в полнокровную группу строк и уезжает
//! в каскад — дальше всё как у PNG.

use std::collections::VecDeque;
use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid::{self, TILE};
use super::super::resample::resample;
use super::{to_rgba, Info, Kind};

/// Потолок области источника, читаемой ради одного тайла (в пикселях).
/// При обычных для COG копиях-половинах область — до 1024², а больше 4096²
/// означает дыру в цепочке копий на четыре уровня: честнее отказать, чем
/// молча прочитать пол-файла.
const REGION_CAP: u64 = 4096 * 4096;

/// Потолок собранного ряда чанков в последовательном проходе. Ряд — это
/// ширина × высота чанка; полосный файл с RowsPerStrip во весь снимок дал бы
/// здесь копию всего растра, а лимит памяти инстанса — 1 ГБ.
const BAND_CAP: u64 = 256 * 1024 * 1024;

/// Бюджет декодированных чанков при прямом доступе: соседние тайлы уровня
/// стоят на одних и тех же чанках источника, и декодировать их по разу на
/// тайл — значит декодировать всё по четыре раза. Бюджет в байтах, а не в
/// штуках: чанки бывают и 256², и 4096², и счёт штуками то не держит ничего,
/// то держит полгигабайта.
const CHUNK_CACHE_BYTES: usize = 128 * 1024 * 1024;

pub struct Layout {
    pub tiled: bool,
    /// Уменьшенные копии (IFD с битом reduced в NewSubfileType), в порядке
    /// обнаружения. В многостраничном TIFF следующие IFD — отдельные
    /// страницы, их сюда не берут — подменять ими первую нельзя.
    pub overviews: Vec<Overview>,
}

pub struct Overview {
    /// Индекс IFD для seek_to_image.
    pub image: usize,
    pub width: u32,
    pub height: u32,
}

impl Layout {
    /// Произвольный тайл дёшев: чанки читаются точечно, а копии закрывают
    /// глубокие уровни. Тайловому файлу без копий верхний тайл стоил бы
    /// чтения всего растра — он идёт последовательным путём.
    pub fn random_access(&self) -> bool {
        self.tiled && !self.overviews.is_empty()
    }
}

pub fn describe<R: Read + Seek>(reader: R) -> Result<Info, String> {
    let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
    let (width, height) = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;
    ensure_chunky(&mut decoder)?;
    let tiled = decoder.get_tag_unsigned::<u32>(Tag::TileWidth).is_ok();

    let mut overviews = Vec::new();
    let mut index = 0;
    while decoder.more_images() {
        if decoder.next_image().is_err() {
            break;
        }
        index += 1;
        let reduced = decoder
            .get_tag_unsigned::<u32>(Tag::NewSubfileType)
            .map(|v| v & 1 != 0)
            .unwrap_or(false);
        if !reduced {
            continue;
        }
        let Ok((w, h)) = decoder.dimensions() else { continue };
        if w == 0 || h == 0 {
            continue;
        }
        overviews.push(Overview { image: index, width: w, height: h });
    }

    Ok(Info { width, height, kind: Kind::Tiff(Layout { tiled, overviews }) })
}

// ── Прямой доступ ──────────────────────────────────────────────

pub fn produce_direct<R: Read + Seek>(
    reader: R,
    info: &Info,
    layout: &Layout,
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;

    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);
    let (image, sw, sh) = pick_source(info, layout, lw);
    decoder.seek_to_image(image).map_err(|e| format!("tiff: {}", e))?;

    // Копии могут быть раскложены иначе, чем базовый IFD, — проверяется та,
    // которую читаем.
    ensure_chunky(&mut decoder)?;
    let channels = channels(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: нулевой размер чанка".to_string());
    }
    // Чанк декодируется целиком, и проверить его габарит надо до чтения:
    // REGION_CAP меряет область тайла, а не то, какими кусками она лежит.
    if u64::from(cw) * u64::from(ch) * 4 > BAND_CAP {
        return Err(format!("tiff: чанк {}×{} не влезает в бюджет памяти", cw, ch));
    }
    let across = sw.div_ceil(cw);
    let mut chunks = ChunkCache::new();

    veldsdk::log::debug!(target: "decode",
        "tiff прямой доступ: уровень {} ({}×{}) из IFD {} ({}×{}), тайлов {}",
        level, lw, lh, image, sw, sh, wants.len());

    for &(tx, ty) in wants {
        let tw = pyramid::tile_extent(tx, lw);
        let th = pyramid::tile_extent(ty, lh);

        // Прямоугольник тайла в пикселях источника. Масштаб дробный, границы
        // наружу (floor/ceil): усреднению нужен каждый задетый пиксель.
        let sx0 = (u64::from(tx) * u64::from(TILE) * u64::from(sw)) / u64::from(lw);
        let sy0 = (u64::from(ty) * u64::from(TILE) * u64::from(sh)) / u64::from(lh);
        let sx1 = (u64::from(tx * TILE + tw) * u64::from(sw)).div_ceil(u64::from(lw)).min(u64::from(sw));
        let sy1 = (u64::from(ty * TILE + th) * u64::from(sh)).div_ceil(u64::from(lh)).min(u64::from(sh));
        let (rw, rh) = ((sx1 - sx0) as u32, (sy1 - sy0) as u32);
        if rw == 0 || rh == 0 {
            return Err(format!("tiff: тайлу {}:{} не досталось пикселей источника", tx, ty));
        }
        if u64::from(rw) * u64::from(rh) > REGION_CAP {
            return Err(format!(
                "tiff: область {}×{} под тайл больше потолка — копии в файле слишком редкие",
                rw, rh
            ));
        }
        let (sx0, sy0) = (sx0 as u32, sy0 as u32);

        // Область собирается из пересечения с чанками; за краем данных нет —
        // у краевых чанков полезная часть короче.
        let mut region = vec![0u8; (rw as usize) * (rh as usize) * 4];
        for cy in sy0 / ch..=(sy0 + rh - 1) / ch {
            for cx in sx0 / cw..=(sx0 + rw - 1) / cw {
                let (data, dw, dh) = chunks.get(&mut decoder, cy * across + cx, channels)?;
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

        let tile = resample(&region, rw, rh, tw, th);
        emit(level, tx, ty, tw, th, &tile)?;
    }
    Ok(())
}

/// Источник для уровня: самая мелкая копия, которой хватает на его ширину.
/// Базовый IFD подходит всегда — уровень не бывает крупнее родного
/// разрешения, поэтому выбор не бывает пустым.
fn pick_source(info: &Info, layout: &Layout, level_width: u32) -> (usize, u32, u32) {
    let mut best = (0usize, info.width, info.height);
    for overview in &layout.overviews {
        if overview.width >= level_width && overview.width < best.1 {
            best = (overview.image, overview.width, overview.height);
        }
    }
    best
}

/// Декодированные чанки текущего IFD, RGBA8. Вытеснение — по старшинству и
/// до бюджета: тайлы приходят соседними, и старее всех — самый ненужный.
struct ChunkCache {
    entries: VecDeque<(u32, Vec<u8>, u32, u32)>,
    bytes: usize,
}

impl ChunkCache {
    fn new() -> Self {
        Self { entries: VecDeque::new(), bytes: 0 }
    }

    fn get<R: Read + Seek>(
        &mut self,
        decoder: &mut Decoder<R>,
        index: u32,
        channels: usize,
    ) -> Result<(&[u8], u32, u32), String> {
        if let Some(at) = self.entries.iter().position(|(i, ..)| *i == index) {
            let (_, data, dw, dh) = &self.entries[at];
            return Ok((data, *dw, *dh));
        }

        let (dw, dh) = decoder.chunk_data_dimensions(index);
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let samples = samples(data)?;
        let rgba = to_rgba(&samples, channels, (dw as usize) * (dh as usize));

        // Свежий остаётся при любом бюджете: без него не собрать текущий тайл.
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

// ── Последовательный проход ────────────────────────────────────

pub fn produce_pass<R: Read + Seek>(reader: R, info: &Info, layout: &Layout, emit: Emit) -> Result<(), String> {
    let mut decoder = Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
    ensure_chunky(&mut decoder)?;
    let channels = channels(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: нулевой размер чанка".to_string());
    }
    let across = info.width.div_ceil(cw);
    let down = info.height.div_ceil(ch);
    if u64::from(info.width) * u64::from(ch) * 4 > BAND_CAP {
        return Err(format!(
            "tiff: ряд чанков {}×{} не влезает в бюджет прохода",
            info.width, ch
        ));
    }

    veldsdk::log::debug!(target: "decode",
        "tiff проход: {}×{}, чанк {}×{}, рядов {} ({})",
        info.width, info.height, cw, ch, down,
        if layout.tiled { "тайловый без копий" } else { "полосный" });

    let mut cascade = Cascade::new(0, info.width, info.height);
    for cy in 0..down {
        // Высота ряда — по первому чанку: у нижнего края ряд короче целиком.
        let rows = decoder.chunk_data_dimensions(cy * across).1;
        let mut band = vec![0u8; (info.width as usize) * (rows as usize) * 4];
        for cx in 0..across {
            let index = cy * across + cx;
            let (dw, dh) = decoder.chunk_data_dimensions(index);
            let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
            let samples = samples(data)?;
            let rgba = to_rgba(&samples, channels, (dw as usize) * (dh as usize));
            let run = (dw as usize) * 4;
            for y in 0..dh.min(rows) as usize {
                let dst = (y * (info.width as usize) + (cx * cw) as usize) * 4;
                band[dst..dst + run].copy_from_slice(&rgba[y * run..(y + 1) * run]);
            }
        }
        cascade.push_rows(&band, rows, emit)?;
    }
    cascade.finish(emit)
}

// ── Общее ──────────────────────────────────────────────────────

/// Планарная раскладка (плоскость на канал) чанкуется по-другому: чанк несёт
/// одну плоскость, а сборка здесь считает его всеми каналами вперемешку.
/// Отказ, а не каша из серых плоскостей.
fn ensure_chunky<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    let planar = decoder.get_tag_unsigned::<u16>(Tag::PlanarConfiguration).unwrap_or(1);
    if planar != 1 {
        return Err("tiff: планарная раскладка сэмплов не поддерживается".to_string());
    }
    Ok(())
}

fn channels(color: tiff::ColorType) -> Result<usize, String> {
    match color {
        tiff::ColorType::Gray(_) => Ok(1),
        tiff::ColorType::GrayA(_) => Ok(2),
        tiff::ColorType::RGB(_) => Ok(3),
        tiff::ColorType::RGBA(_) => Ok(4),
        other => Err(format!("tiff: цветовая модель {:?} не поддерживается", other)),
    }
}

/// Сэмплы чанка в 8 битах на канал. У 16-битных снимков берётся старший байт:
/// для тайлов этого достаточно, а радиометрическое растягивание — отдельная
/// задача, и придумывать её за пользователя тут нечего.
fn samples(data: DecodingResult) -> Result<Vec<u8>, String> {
    use tiff::decoder::DecodingResult as R;
    match data {
        R::U8(v) => Ok(v),
        R::U16(v) => Ok(v.into_iter().map(|s| (s >> 8) as u8).collect()),
        other => Err(format!("tiff: разрядность сэмплов не поддерживается ({})", match other {
            R::U32(_) => "u32",
            R::U64(_) => "u64",
            R::F32(_) => "f32",
            R::F64(_) => "f64",
            _ => "знаковая",
        })),
    }
}
