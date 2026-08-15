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
//!
//! Сэмплы шире байта (u16, i16, f32 — радар, DEM) идут в RGBA через растяг по
//! выборке файла, «нет данных» — прозрачностью; правила — в radiometry.rs,
//! здесь только выбор выборки (см. [`mapping`]).

use std::collections::VecDeque;
use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid::{self, TILE};
use super::super::resample::resample;
use super::radiometry::{percentile_stretch, Mapping, Samples};
use super::{Info, Kind, Tie};

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
    let ties = ties(&mut decoder, width, height);

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

    Ok(Info { width, height, kind: Kind::Tiff(Layout { tiled, overviews }), ties })
}

/// Сетка геопривязки GeoTIFF — опорные точки в градусах. Пусто, если файл
/// привязан к проекции, а не к градусам: перевести её мог бы только тот, кто
/// знает саму проекцию, а тайлер знает про растр и не знает про Землю.
///
/// Два вида привязки, и оба сводятся к одному: решётка точек (ModelTiepoint по
/// шесть чисел — пиксель, потом место) — как есть; одна точка с шагом пикселя
/// (ModelPixelScale) — четырьмя углами, потому что такой растр лежит в градусах
/// ровным прямоугольником и промежуточные точки в нём линейны.
///
/// Декодер после возврата стоит на том же образе: наводки здесь нет.
fn ties<R: Read + Seek>(decoder: &mut Decoder<R>, width: u32, height: u32) -> Vec<Tie> {
    let keys = decoder.get_tag_u16_vec(Tag::GeoKeyDirectoryTag).unwrap_or_default();
    let points = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).unwrap_or_default();
    let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).unwrap_or_default();
    geo_ties(&keys, &points, &scale, width, height)
}

/// Разбор геотегов — отдельно от чтения, потому что проверяется он ими же:
/// файла с решёткой в тестах нет, а правила «градусы или проекция», «шесть
/// чисел на узел» и «шаг по Y идёт на юг» есть.
fn geo_ties(keys: &[u16], points: &[f64], scale: &[f64], width: u32, height: u32) -> Vec<Tie> {
    // GTModelTypeGeoKey (1024): 2 — градусы. Ключи лежат четвёрками после
    // заголовка из четырёх же чисел; значение простого ключа (место 0) —
    // четвёртое в четвёрке.
    let geographic = keys
        .get(4..)
        .unwrap_or_default()
        .chunks_exact(4)
        .any(|key| key[0] == 1024 && key[1] == 0 && key[3] == 2);
    if !geographic {
        return Vec::new();
    }

    let point = |px: f64, py: f64, lon: f64, lat: f64| Tie { px, py, lat, lon };
    // Узел — шесть чисел: пиксель (i, j, k) и место (x, y, z).
    if points.len() > 6 {
        return points.chunks_exact(6).map(|tie| point(tie[0], tie[1], tie[3], tie[4])).collect();
    }

    // Одна точка с шагом пикселя: растр лежит в градусах ровным
    // прямоугольником, и хватает его углов.
    let (Some(tie), true) = (points.get(..6), scale.len() >= 2) else {
        return Vec::new();
    };
    // Шаг по Y положителен, а строки растра идут на юг — отсюда минус.
    let (x, y) = (tie[3] - tie[0] * scale[0], tie[4] + tie[1] * scale[1]);
    let (right, bottom) = (f64::from(width), f64::from(height));
    vec![
        point(0.0, 0.0, x, y),
        point(right, 0.0, x + right * scale[0], y),
        point(0.0, bottom, x, y - bottom * scale[1]),
        point(right, bottom, x + right * scale[0], y - bottom * scale[1]),
    ]
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

    // Выборка растяга — из самой мелкой копии: она дешёвая и одна на все
    // уровни, а значит один и тот же растяг у всех тайлов файла.
    let stats = layout.overviews.iter().min_by_key(|o| o.width).map_or(0, |o| o.image);
    let mapping = mapping(&mut decoder, stats)?;

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
                let (data, dw, dh) = chunks.get(&mut decoder, cy * across + cx, channels, &mapping)?;
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
        mapping: &Mapping,
    ) -> Result<(&[u8], u32, u32), String> {
        if let Some(at) = self.entries.iter().position(|(i, ..)| *i == index) {
            let (_, data, dw, dh) = &self.entries[at];
            return Ok((data, *dw, *dh));
        }

        let (dw, dh) = decoder.chunk_data_dimensions(index);
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let rgba = mapping.rgba(&typed(&data)?, channels, (dw as usize) * (dh as usize));

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
    // Копий нет — выборка растяга из базового IFD; байтовым файлам она не
    // стоит ни одного лишнего чтения.
    let mapping = mapping(&mut decoder, 0)?;
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
            let rgba = mapping.rgba(&typed(&data)?, channels, (dw as usize) * (dh as usize));
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

/// Сэмплы чанка в типизированном виде — то, что умеет разложить показ.
/// Остальные разрядности — отказ: файла с ними в каталоге не встречалось,
/// а поддержка «на всякий случай» не проверяется ничем.
fn typed(data: &DecodingResult) -> Result<Samples<'_>, String> {
    use tiff::decoder::DecodingResult as R;
    Ok(match data {
        R::U8(v) => Samples::U8(v),
        R::U16(v) => Samples::U16(v),
        R::I16(v) => Samples::I16(v),
        R::F32(v) => Samples::F32(v),
        other => {
            return Err(format!(
                "tiff: разрядность сэмплов не поддерживается ({})",
                match other {
                    R::U32(_) => "u32",
                    R::U64(_) => "u64",
                    R::F16(_) => "f16",
                    R::F64(_) => "f64",
                    _ => "знаковая",
                }
            ))
        }
    })
}

/// Сколько значений хватает выборке растяга на файл: перцентили по миллиону
/// не отличимы от перцентилей по всем, а сортировка остаётся мгновенной.
const STAT_SAMPLES: usize = 1 << 20;

/// Маппинг показа файла: байтам — тождество, широким форматам — растяг
/// перцентилей (см. radiometry.rs). Выборка — до четырёх чанков вразброс из
/// IFD `stats` (у COG — самая мелкая копия, у прохода — базовый), прорежена
/// до [`STAT_SAMPLES`]. Выбор детерминирован: одному файлу — один растяг,
/// какие тайлы и в каком порядке ни спроси.
///
/// Декодер после возврата стоит на IFD `stats` — вызывающий сам наводит его
/// на нужный образ.
fn mapping<R: Read + Seek>(decoder: &mut Decoder<R>, stats: usize) -> Result<Mapping, String> {
    // GDAL_NODATA пишется в базовый IFD — читается до пере-наводки.
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim().trim_end_matches('\0').parse::<f32>().ok());
    let bits = match decoder.colortype().map_err(|e| format!("tiff: {}", e))? {
        tiff::ColorType::Gray(bits)
        | tiff::ColorType::GrayA(bits)
        | tiff::ColorType::RGB(bits)
        | tiff::ColorType::RGBA(bits) => bits,
        other => return Err(format!("tiff: цветовая модель {:?} не поддерживается", other)),
    };
    if bits <= 8 {
        return Ok(Mapping::identity(nodata));
    }

    decoder.seek_to_image(stats).map_err(|e| format!("tiff: {}", e))?;
    ensure_chunky(decoder)?;
    let (w, h) = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: нулевой размер чанка".to_string());
    }
    if u64::from(cw) * u64::from(ch) * 4 > BAND_CAP {
        return Err(format!("tiff: чанк {}×{} не влезает в бюджет памяти", cw, ch));
    }

    let total = w.div_ceil(cw) * h.div_ceil(ch);
    let mut picks = [0, total / 3, 2 * total / 3, total.saturating_sub(1)].to_vec();
    picks.dedup();

    let mut values = Vec::new();
    for &index in &picks {
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let samples = typed(&data)?;
        let step = (samples.len() * picks.len() / STAT_SAMPLES).max(1);
        for i in (0..samples.len()).step_by(step) {
            let v = samples.get(i);
            if v.is_finite() && Some(v) != nodata {
                values.push(v);
            }
        }
    }
    match percentile_stretch(&mut values) {
        Some((lo, hi)) => Ok(Mapping::stretched(lo, hi, nodata)),
        // Вся выборка — «нет данных»: растягивать нечего, а прозрачным файл
        // сделает само ключевание.
        None => Ok(Mapping::identity(nodata)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Заголовок каталога геоключей: версия, ревизия и число ключей.
    fn geokeys(model: u16) -> Vec<u16> {
        vec![1, 1, 0, 1, 1024, 0, 1, model]
    }

    /// Решётка узлов проходит как есть: пиксель берётся из первых двух чисел
    /// шестёрки, место — из четвёртого и пятого. Порядок узлов — файла: им и
    /// сказано, каким пикселем куда лёг растр.
    #[test]
    fn tiepoint_lattice_keeps_pixel_and_place() {
        // Два узла верхнего ребра гранулы Sentinel-1: слева восток, справа
        // запад — снимок нисходящего витка лежит поперёк меридианов.
        let points = vec![
            0.0, 0.0, 0.0, 2.707, 73.395, 0.0, //
            529.0, 0.0, 0.0, 2.096, 73.470, 0.0,
        ];
        let ties = geo_ties(&geokeys(2), &points, &[], 10572, 9993);
        assert_eq!(ties.len(), 2);
        assert_eq!((ties[0].px, ties[0].py), (0.0, 0.0));
        assert_eq!((ties[0].lat, ties[0].lon), (73.395, 2.707));
        assert_eq!((ties[1].px, ties[1].py), (529.0, 0.0));
    }

    /// Одна точка с шагом пикселя — четырьмя углами, и строки идут на юг:
    /// нижний край южнее верхнего, а не наоборот.
    #[test]
    fn pixel_scale_becomes_four_corners_facing_south() {
        let points = vec![0.0, 0.0, 0.0, 10.0, 50.0, 0.0];
        let ties = geo_ties(&geokeys(2), &points, &[0.5, 0.25, 0.0], 100, 200);
        assert_eq!(ties.len(), 4);
        assert_eq!((ties[0].lon, ties[0].lat), (10.0, 50.0));
        assert_eq!((ties[1].lon, ties[1].lat), (60.0, 50.0), "правый край восточнее");
        assert_eq!((ties[2].lon, ties[2].lat), (10.0, 0.0), "нижний край южнее");
    }

    /// Привязка к проекции — не наше дело: перевести её в градусы может только
    /// тот, кто знает саму проекцию, а тайлер про Землю не знает ничего.
    #[test]
    fn projected_files_yield_nothing() {
        let points = vec![0.0, 0.0, 0.0, 600_000.0, 7_800_000.0, 0.0];
        assert!(geo_ties(&geokeys(1), &points, &[10.0, 10.0, 0.0], 10, 10).is_empty());
        // Как и файл вовсе без геотегов.
        assert!(geo_ties(&[], &points, &[10.0, 10.0, 0.0], 10, 10).is_empty());
    }
}
