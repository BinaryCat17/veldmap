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
use super::super::resample::{resample_window, Window};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples};
use super::{Info, Kind, Placement, Tie};

/// Сигнатуры BigTIFF: у него в заголовке стоит версия 43 вместо 42, и по этому
/// числу его и узнают. Смотрится она здесь, рядом с JP2 и NetCDF, потому что
/// крейт `image` знает только классические сигнатуры и отвечает «это не
/// изображение» — тогда как крейт `tiff` такой файл читает наравне с обычным.
pub const BIG_MAGIC: [&[u8]; 2] = [b"II\x2b\x00", b"MM\x00\x2b"];

/// Потолок области источника, читаемой ради одного тайла (в пикселях).
/// При обычных для COG копиях-половинах уровень читается из своей копии, и
/// область — чуть больше тайла, до 513². Перевалить за 4096² она может, когда
/// ближайшая годная копия крупнее уровня в шестнадцать раз по стороне:
/// честнее отказать, чем молча прочитать пол-файла.
///
/// Годная — по обеим сторонам (см. [`pick_source`]), поэтому дорог сюда два:
/// пропуск четырёх уровней в цепочке копий и вытянутый растр, у которого
/// короткая сторона уровня в один-два пикселя, — там отбрасывается всякая
/// копия, и уровень читается из базового IFD.
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
    ensure_readable(&mut decoder)?;
    let tiled = decoder.get_tag_unsigned::<u32>(Tag::TileWidth).is_ok();
    let (ties, placement) = georef(&mut decoder, width, height);

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
        overviews.push(Overview { image: index, width: w, height: h });
    }

    Ok(Info {
        width,
        height,
        kind: Kind::Tiff(Layout { tiled, overviews }),
        finest: 0,
        ties,
        placement,
        // Отсчёт прибора объявляет один Sentinel-3 своими глобальными
        // атрибутами; у GeoTIFF место записано самим растром.
        frame: None,
    })
}

/// Копия ли это картинки, ужатая, — по тегу NewSubfileType.
///
/// Мало «уменьшенной» (бит 1): маска прозрачности помечается битом 4, и её
/// собственные копии несут 5 = 1|4 — так их пишет GDAL, и такой копией
/// оказывается однобитная маска вместо снимка. Пирамида берёт копию по
/// размеру, поэтому чужая среди своих не отвергается ниже по течению, а
/// показывается.
fn is_overview(subfile_type: u32) -> bool {
    const REDUCED: u32 = 1;
    const MASK: u32 = 4;
    subfile_type & REDUCED != 0 && subfile_type & MASK == 0
}

/// Геопривязка GeoTIFF: узлы в градусах либо привязка к проекции. Одно из
/// двух, потому что и записана в файле она одна — градусы или метры системы, —
/// а смешать их значило бы соврать числами, а не промолчать.
///
/// Градусы разбирает [`geo_ties`], проекцию — [`geo_placement`]; здесь только
/// чтение тегов и то, о чём надо сказать вслух.
///
/// Декодер после возврата стоит на том же образе: наводки здесь нет.
fn georef<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    width: u32,
    height: u32,
) -> (Vec<Tie>, Option<Placement>) {
    let keys = decoder.get_tag_u16_vec(Tag::GeoKeyDirectoryTag).unwrap_or_default();
    let points = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).unwrap_or_default();
    let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).unwrap_or_default();
    // Матрицу крейт тегом не знает: у него перечислены только ходовые. Номер
    // из спецификации GeoTIFF — ModelTransformationTag.
    let matrix = decoder.get_tag_f64_vec(Tag::Unknown(34264)).unwrap_or_default();

    if let Some(code) = foreign_datum(&keys) {
        veldsdk::log::warn!(target: "decode",
            "растр объявляет датум EPSG:{}, а привязка уедет как WGS84: расхождение порядка сотни метров",
            code);
    }
    let placement = geo_placement(&keys, &points, &scale, &matrix);
    let ties = geo_ties(&keys, &points, &scale, &matrix, width, height);
    // Сказать надо именно здесь: дальше по течению «в файле не сказано» и
    // «сказано, да не прочиталось» выглядят одинаково — пустой привязкой, — и
    // объяснить по такой пустоте нечего.
    //
    // Условие — по тегам привязки, а не по модели координат: молчаливых исходов
    // столько же у геоцентрики (1024 = 3) и у user-defined, сколько у проекции,
    // и названный род оставил бы их всех без объяснения.
    let carries = !points.is_empty() || !scale.is_empty() || !matrix.is_empty();
    if carries && ties.is_empty() && placement.is_none() {
        veldsdk::log::warn!(target: "decode",
            "растр несёт привязку, а взять её не удалось: модель {:?}, система {:?}, опорных точек {}",
            geokey(&keys, 1024), geokey(&keys, 3072), points.len() / 6);
    }
    (ties, placement)
}

/// Значение простого геоключа: ключи лежат четвёрками после заголовка из
/// четырёх же чисел, и у простого (место 0) значение — четвёртое в четвёрке.
fn geokey(keys: &[u16], id: u16) -> Option<u16> {
    keys.get(4..)
        .unwrap_or_default()
        .chunks_exact(4)
        .find(|entry| entry[0] == id && entry[1] == 0)
        .map(|entry| entry[3])
}

/// Сдвиг узла, если координата названа для середины пикселя, а не для его угла
/// (GTRasterTypeGeoKey, 1025). Наружу узел уезжает долей растра, где ноль —
/// край, и такой узел приходится сдвигать на полпикселя. Умолчание
/// спецификации — угол, и тогда сдвига нет.
///
/// Полпикселя — это не мелочь у растров, которыми меряют: у Copernicus DSM шаг
/// секундный, и половина его — пятнадцать метров на местности.
fn half_pixel(keys: &[u16]) -> f64 {
    if geokey(keys, 1025) == Some(2) { 0.5 } else { 0.0 }
}

/// Датум, объявленный файлом, — если он не WGS84 (GeographicTypeGeoKey, 2048).
///
/// Спрашивают об этом затем, что дальше по течению места для датума нет: и
/// узлы, и рамка объявлены в WGS84, так что файл на Пулково-42 (EPSG:4284)
/// уехал бы туда молча — а это сотня метров в средней полосе. Числа отсюда не
/// правятся: перевод между датумами — ещё одна геодезия, и заводить её ради
/// одной строки нельзя. Строка и есть весь ответ.
///
/// Молчание файла и `user-defined` — не чужой датум, а отсутствие ответа:
/// сказать о них нечего.
fn foreign_datum(keys: &[u16]) -> Option<u16> {
    const WGS84: [u16; 2] = [4326, 4979];
    const UNSAID: [u16; 2] = [0, 32767];
    geokey(keys, 2048).filter(|code| !WGS84.contains(code) && !UNSAID.contains(code))
}

/// Годен ли шаг пикселя: обе стороны — настоящие числа и ненулевые. Нулевая
/// сложила бы растр в линию, а такая привязка снаружи неотличима от настоящей —
/// её нечем поймать ни по числу узлов, ни по их порядку.
///
/// Не-число проверяется отдельно, потому что сравнение с нулём его пропускает:
/// `NaN != 0` истинно.
fn usable_step(scale: &[f64]) -> bool {
    matches!(scale.get(..2), Some([x, y]) if x.is_finite() && y.is_finite() && *x != 0.0 && *y != 0.0)
}

/// Привязка растра, лежащего в проекции: код EPSG и линейное преобразование
/// пикселя в метры системы.
///
/// `None` — файл лежит в градусах (тогда его читает [`geo_ties`]), система не
/// названа, названа непонятным кодом либо преобразование вырождено.
///
/// Решётка опорных точек означает здесь отказ, а не привязку по первому узлу:
/// решётка описывает нелинейную раскладку — тем она и решётка, — и шестёркой
/// чисел не выражается. Взятая по одному узлу, она врала бы тем сильнее, чем
/// дальше от него.
///
/// Обрезанный тег опорной точки (меньше шести чисел) сюда не попадает вовсе и
/// уходит в матричную ветку — а не имея матрицы, кончается отказом. Читать
/// половину узла и достраивать вторую было бы догадкой, а не чтением.
fn geo_placement(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
) -> Option<Placement> {
    // GTModelTypeGeoKey (1024): 1 — метры проекции.
    if geokey(keys, 1024) != Some(1) || points.len() > 6 {
        return None;
    }
    // ProjectedCSTypeGeoKey (3072). Ноль и user-defined кодом не являются:
    // параметры такой системы лежат отдельными ключами, и собрать её из них —
    // это уже разбор проекций, а не чтение растра.
    let epsg = match geokey(keys, 3072) {
        Some(code) if code != 0 && code != 32767 => u32::from(code),
        _ => return None,
    };

    let half = half_pixel(keys);
    let affine = match (points.get(..6), usable_step(scale)) {
        // Шаг по Y положителен, а строки растра идут на юг — отсюда минус.
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
    // Опорная точка проверяется вместе со всем остальным: шаг мог быть годен, а
    // место названо не числом, и такая рамка доехала бы до глобуса целой с виду.
    affine.iter().all(|value| value.is_finite()).then_some(Placement { epsg, affine })
}

/// Опорные точки растра, лежащего в градусах. Пусто, если файл привязан к
/// проекции: перевести её мог бы только тот, кто знает саму проекцию, а тайлер
/// знает про растр и не знает про Землю (см. [`geo_placement`]).
///
/// Разбор геотегов отделён от чтения, потому что проверяется он ими же: файла
/// с решёткой в тестах нет, а правила «градусы или проекция», «шесть чисел на
/// узел» и «шаг по Y идёт на юг» есть.
fn geo_ties(
    keys: &[u16],
    points: &[f64],
    scale: &[f64],
    matrix: &[f64],
    width: u32,
    height: u32,
) -> Vec<Tie> {
    // GTModelTypeGeoKey (1024): 2 — градусы.
    if geokey(keys, 1024) != Some(2) {
        return Vec::new();
    }
    let half = half_pixel(keys);

    let point = |px: f64, py: f64, lon: f64, lat: f64| Tie { px, py, lat, lon };
    // Узел — шесть чисел: пиксель (i, j, k) и место (x, y, z).
    if points.len() > 6 {
        return points
            .chunks_exact(6)
            .map(|tie| point(tie[0] + half, tie[1] + half, tie[3], tie[4]))
            .collect();
    }

    // Одна точка с шагом пикселя: растр лежит в градусах ровным
    // прямоугольником, и хватает его углов.
    let (Some(tie), true) = (points.get(..6), usable_step(scale)) else {
        return corners_from_matrix(matrix, half, width, height);
    };
    // Шаг по Y положителен, а строки растра идут на юг — отсюда минус.
    let (x, y) = (tie[3] - (tie[0] + half) * scale[0], tie[4] + (tie[1] + half) * scale[1]);
    let (right, bottom) = (f64::from(width), f64::from(height));
    vec![
        point(0.0, 0.0, x, y),
        point(right, 0.0, x + right * scale[0], y),
        point(0.0, bottom, x, y - bottom * scale[1]),
        point(right, bottom, x + right * scale[0], y - bottom * scale[1]),
    ]
}

/// Аффинное преобразование из матрицы привязки (ModelTransformationTag).
///
/// Матрица — четыре строки по четыре числа, и место пикселя (i, j) даёт первая
/// пара строк: `x = a·i + b·j + d`, `y = e·i + f·j + h`. Такая привязка стои́т
/// вместо пары «точка + шаг» и тем от неё отличается, что растр может лежать
/// повёрнутым: у пары шаг задан по осям, повернуть его нечем.
///
/// `None` — вырожденная матрица (нулевая строка, единичная заглушка). Привязкой
/// она не является: все четыре угла сошлись бы в точку, и растр лёг бы в ничто.
///
/// Полпикселя середины уходит в свободный член, а не снимается с довода на
/// каждом обращении: наружу отсюда уезжает одно преобразование, и второй
/// конвенции у него быть не должно.
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

/// Углы растра, привязанного матрицей, — в градусах.
///
/// Углов хватает и повёрнутому: преобразование линейное, а между четырьмя
/// узлами решётка и восстанавливает линейное точно.
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
    let (image, sw, sh) = pick_source(info, layout, lw, lh);
    decoder.seek_to_image(image).map_err(|e| format!("tiff: {}", e))?;

    // Копии могут быть раскложены иначе, чем базовый IFD, — проверяется та,
    // которую читаем.
    let (cw, ch) = chunk_grid(&mut decoder)?;
    let pixel = pixel(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
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
        //
        // Прочитанное при этом ШИРЕ того, что тайлу принадлежит, — на долю
        // пикселя с каждой стороны. Тайлу принадлежит окно `window` ниже, и
        // ужимается именно оно: растянутое на тайл прочитанное целиком уехало
        // бы на эту долю, у соседнего тайла — в другую сторону, и на стыке
        // остался бы шов. У двоичных копий доли нулевые и разницы нет, а
        // небинарные (3, 5, 7/2) дают её на каждом тайле.
        let exact = |at: u64, side: u64, level_side: u64| f64::from(at as u32) * side as f64 / level_side as f64;
        let sx0 = (u64::from(tx) * u64::from(TILE) * u64::from(sw)) / u64::from(lw);
        let sy0 = (u64::from(ty) * u64::from(TILE) * u64::from(sh)) / u64::from(lh);
        let sx1 = (u64::from(tx * TILE + tw) * u64::from(sw)).div_ceil(u64::from(lw)).min(u64::from(sw));
        let sy1 = (u64::from(ty * TILE + th) * u64::from(sh)).div_ceil(u64::from(lh)).min(u64::from(sh));
        let window = Window {
            x0: exact(u64::from(tx) * u64::from(TILE), u64::from(sw), u64::from(lw)) - sx0 as f64,
            y0: exact(u64::from(ty) * u64::from(TILE), u64::from(sh), u64::from(lh)) - sy0 as f64,
            x1: exact(u64::from(tx * TILE + tw), u64::from(sw), u64::from(lw)) - sx0 as f64,
            y1: exact(u64::from(ty * TILE + th), u64::from(sh), u64::from(lh)) - sy0 as f64,
        };
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
                let (data, dw, dh) = chunks.get(&mut decoder, cy * across + cx, pixel, &mapping)?;
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

        let tile = resample_window(&region, rw, rh, window, tw, th);
        emit(level, tx, ty, tw, th, &tile)?;
    }
    Ok(())
}

/// Источник для уровня: самая мелкая копия, которой хватает на обе его
/// стороны.
///
/// Хватает — с точностью до округления. Сторону уровня считают округлением
/// вверх (`pyramid::level_size`), а копии в файле записаны делением вниз, и
/// у нечётной стороны эти два счёта расходятся ровно на пиксель: у растра
/// 25437 уровню 1 нужно 12719, а его же копия в файле — 12718. Требовать
/// копию не у́же уровня значит отвергнуть её и взять вдвое крупнее, то есть
/// прочитать вчетверо больше пикселей на каждый тайл.
///
/// Прощёный пиксель — это дорисовка в один столбец на весь тайл
/// (`resample_window` разворачивает окно шире, чем оно есть, с коэффициентом
/// `сторона уровня / сторона копии`). Больше пикселя прощать нельзя: у
/// короткой стороны пиксель — это уже разы, и растр 513×3 собирал бы уровень
/// высотой 2 из копии высотой 1.
///
/// Прощается только округление, поэтому обе стороны проверяются порознь, а
/// копия мельче половины уровня не годится никогда. Второе условие и держит
/// короткую сторону: у стороны в два пикселя «пиксель разницы» — это её
/// половина, и такая копия не округлена, а потеряна. Сработать оно может
/// только на стороне в один-два пикселя, потому что `2·(n−1) > n` при всяком
/// `n` больше двух; заодно им же отсекается копия под нулевой уровень —
/// округлять там нечего.
fn pick_source(
    info: &Info,
    layout: &Layout,
    level_width: u32,
    level_height: u32,
) -> (usize, u32, u32) {
    // Вычитание из стороны уровня, а не прибавление к стороне копии: битый
    // IFD с шириной у потолка u32 переполнил бы её вместе со сборкой.
    let fits = |copy: u32, level: u32| {
        copy >= level.saturating_sub(1) && u64::from(copy) * 2 > u64::from(level)
    };

    let mut best = (0usize, info.width, info.height);
    for overview in &layout.overviews {
        if fits(overview.width, level_width)
            && fits(overview.height, level_height)
            && overview.width < best.1
        {
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
        pixel: Pixel,
        mapping: &Mapping,
    ) -> Result<(&[u8], u32, u32), String> {
        if let Some(at) = self.entries.iter().position(|(i, ..)| *i == index) {
            let (_, data, dw, dh) = &self.entries[at];
            return Ok((data, *dw, *dh));
        }

        let (dw, dh) = decoder.chunk_data_dimensions(index);
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let rgba = chunk_rgba(mapping, &data, pixel, dw, dh)?;

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
    let (cw, ch) = chunk_grid(&mut decoder)?;
    let pixel = pixel(decoder.colortype().map_err(|e| format!("tiff: {}", e))?)?;
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
            let rgba = chunk_rgba(&mapping, &data, pixel, dw, dh)?;
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
/// Размер чанка у образа, на котором стои́т декодер, — вместе с проверками, без
/// которых его нельзя читать.
///
/// Спрашивают это все три пути: прямой доступ, последовательный проход и
/// выборка растяга. Нужно им одно и то же — раскладка обязана быть
/// интерливленной, размер чанка ненулевым, а сам чанк влезать в память, потому
/// что декодируется он целиком. Проверка габарита стои́т до чтения и меряет
/// именно чанк: `REGION_CAP` меряет область тайла, а не то, какими кусками она
/// лежит.
fn chunk_grid<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(u32, u32), String> {
    ensure_chunky(decoder)?;
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err("tiff: нулевой размер чанка".to_string());
    }
    if u64::from(cw) * u64::from(ch) * 4 > BAND_CAP {
        return Err(format!("tiff: чанк {}×{} не влезает в бюджет памяти", cw, ch));
    }
    Ok((cw, ch))
}

fn ensure_chunky<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    let planar = decoder.get_tag_unsigned::<u16>(Tag::PlanarConfiguration).unwrap_or(1);
    if planar != 1 {
        return Err("tiff: планарная раскладка сэмплов не поддерживается".to_string());
    }
    Ok(())
}

/// Чанк в RGBA — с проверкой, что он вышел целым.
///
/// Растяг укорачивает выход молча, когда сэмплов пришло меньше, чем обещает
/// размер чанка, а сборка и тайла, и полосы блитит по обещанному. Обрезанный
/// чанк битого файла обязан кончиться строкой, а не выходом за границу среза:
/// последнее для wasm-модуля — трап, после которого хост поднимает инстанс
/// заново, а заказчик остаётся ждать ответа, которого уже никто не пришлёт.
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
            "tiff: чанк {}×{} пришёл неполным — {} пикселей вместо {}",
            dw,
            dh,
            rgba.len() / 4,
            pixels
        )),
    }
}

/// Цветовая модель растра числами: сколько сэмплов на пиксель и сколько бит на
/// сэмпл. Один разбор на всех, кто об этом спрашивает: список принятых моделей
/// иначе жил бы в двух видах и разошёлся бы молча.
fn model(color: tiff::ColorType) -> Result<(Pixel, u8), String> {
    match color {
        tiff::ColorType::Gray(bits) => Ok((Pixel::named(1), bits)),
        tiff::ColorType::GrayA(bits) => Ok((Pixel::named(2), bits)),
        tiff::ColorType::RGB(bits) => Ok((Pixel::named(3), bits)),
        tiff::ColorType::RGBA(bits) => Ok((Pixel::named(4), bits)),
        // Так крейт называет всё, у чего сэмплов больше одного, а цветовой
        // интерпретации нет, — то есть обычный вывод GDAL для любого стека
        // полос. Это не «неподдерживаемая модель», а стопка измеренных величин:
        // показывается первая, остальные шагаются мимо (см. `Pixel::stack`).
        tiff::ColorType::Multiband { bit_depth, num_samples } => {
            Ok((Pixel::stack(usize::from(num_samples)), bit_depth))
        }
        other => Err(format!("tiff: цветовая модель {:?} не поддерживается", other)),
    }
}

fn pixel(color: tiff::ColorType) -> Result<Pixel, String> {
    Ok(model(color)?.0)
}

/// Отказ на том, что иначе упало бы посреди прохода.
///
/// Спрашивается это при описании, а не при производстве, потому что цветовая
/// модель и разрядность у файла одни на все его тайлы: отказ здесь — честное
/// «смотреть не на что», а тот же отказ на первом тайле выглядел бы как
/// «кадр неполон» и предлагал бы переспросить.
///
/// Разрядность меньше байта названа отдельно: сэмплы такой глубины крейт не
/// распаковывает, а отдаёт как они лежат — по нескольку в байте, — тогда как
/// сборка тайла считает, что на пиксель приходится целое их число. Раньше это
/// был не отказ, а выход за границу буфера, то есть трап всего инстанса.
fn ensure_readable<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(), String> {
    let color = decoder.colortype().map_err(|e| format!("tiff: {}", e))?;
    let (_, bits) = model(color)?;
    match bits < 8 {
        true => Err(format!(
            "tiff: {} бит на сэмпл — такая разрядность не разворачивается в пиксели",
            bits
        )),
        false => Ok(()),
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

/// Маппинг показа файла: байтам — тождество, широким форматам — растяг
/// перцентилей (см. radiometry.rs). Выборка — до четырёх чанков вразброс из
/// IFD `stats` (у COG — самая мелкая копия, у прохода — базовый), прорежена
/// до [`radiometry::STRETCH_SAMPLES`]. Выбор детерминирован: одному файлу — один растяг,
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
        // Выборка идёт по пикселям и берёт из каждого только цветовые сэмплы:
        // в кадр идут ровно они (см. `Mapping::rgba`), а альфа и
        // непоказываемые полосы двигали бы перцентили, ни на что не влияя.
        // Разница не тонкая: у серого с альфой постоянная альфа 65535 — это
        // половина выборки на верхнем краю формата, и снимок выходит почти
        // чёрным; в стопке полос GDAL маска 0..1 рядом с амплитудой утягивает
        // нижний край в ноль.
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
        // Вся выборка — «нет данных»: растягивать не по чему, и выдумывать
        // растяг нельзя. Любой назначенный сюда предел белит всё, что выше
        // него: у растяга [0, 1] белым выходит уже единица. Значения поэтому
        // принимаются за байты — единственное, чего мы не выдумывали, — и для
        // байтового растра это ровно верно, а у широкого кадр и так почти
        // весь прозрачен: выборка в миллион отсчётов не нашла в нём ни одного
        // годного (см. `Mapping::identity`).
        None => Ok(Mapping::identity(nodata)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Растр без копий — под ним `pick_source` смотрит только на размеры.
    fn bare(width: u32, height: u32) -> Info {
        Info::plain(width, height, Kind::Tiff(Layout { tiled: true, overviews: Vec::new() }))
    }

    /// Копии настоящего снимка — гранула Sentinel-1 GRDH
    /// `S1C_IW_GRDH_1SDV_…_COG.SAFE`, растр 26553×16668, шесть IFD.
    ///
    /// Числа сняты с файла, а не выведены: правило выбора стои́т на том, как
    /// пишет стороны копий GDAL, и проверять эту модель ею же бессмысленно.
    /// Три нижних IFD (1659×1041, 829×520, 414×260) прочитаны из строки
    /// описания, три верхних продолжают ту же цепочку.
    fn sentinel1_grdh() -> (Info, Layout) {
        let overviews = [
            (13276, 8334),
            (6638, 4167),
            (3319, 2083),
            (1659, 1041),
            (829, 520),
            (414, 260),
        ];
        let overviews = overviews
            .iter()
            .enumerate()
            .map(|(step, &(width, height))| Overview { image: step + 1, width, height })
            .collect();
        (bare(26553, 16668), Layout { tiled: true, overviews })
    }

    /// Копии, записанные делением стороны пополам вниз, — так их пишет GDAL.
    fn halved_down(width: u32, height: u32, count: usize) -> Layout {
        let (mut w, mut h) = (width, height);
        let overviews = (1..=count)
            .map(|image| {
                w /= 2;
                h /= 2;
                Overview { image, width: w, height: h }
            })
            .collect();
        Layout { tiled: true, overviews }
    }

    /// Копия годится своему уровню, хотя на пиксель у́же его. Проверять надо
    /// на нечётной стороне: у чётной оба счёта совпадают, и правило на ней не
    /// проверяется вовсе.
    ///
    /// Это и есть та пара, которая обязана сойтись механически: уровень
    /// считает `pyramid::level_size` округлением вверх, копии в файле — GDAL
    /// делением вниз. Разойдясь, они стоят чтения вчетверо большей копии на
    /// каждом уровне, кроме нулевого.
    #[test]
    fn копия_годится_своему_уровню_у_нечётной_стороны() {
        // Sentinel-1 GRDH: 25437×16729, шесть IFD.
        let (width, height) = (25437u32, 16729u32);
        let layout = halved_down(width, height, 5);
        let info = bare(width, height);

        for level in 1..=5usize {
            let (lw, lh) = (
                pyramid::level_size(width, level as u32),
                pyramid::level_size(height, level as u32),
            );
            let (image, chosen, chosen_h) = pick_source(&info, &layout, lw, lh);
            assert_eq!(
                (image, chosen, chosen_h),
                (level, lw - 1, lh - 1),
                "уровню {} ({}×{}) досталась чужая копия",
                level, lw, lh
            );
        }
    }

    /// То же на снятых с настоящего файла числах, а не на выведенных нашей
    /// же моделью деления.
    ///
    /// Ширина у гранулы нечётная, поэтому расходится она на каждом уровне —
    /// это и есть промах, ради которого допуск заведён. Высота делится на
    /// четыре, и на первых двух уровнях расхождения нет вовсе: допуск нужен
    /// не всегда и не всем сторонам сразу.
    #[test]
    fn у_настоящей_гранулы_каждый_уровень_берёт_свою_копию() {
        let (info, layout) = sentinel1_grdh();

        for level in 1..=6usize {
            let (lw, lh) = (
                pyramid::level_size(info.width, level as u32),
                pyramid::level_size(info.height, level as u32),
            );
            let copy = &layout.overviews[level - 1];
            assert_eq!(lw - copy.width, 1, "ширина уровня {} и его копии", level);
            assert!(lh - copy.height <= 1, "высота уровня {} и его копии", level);
            assert_eq!(pick_source(&info, &layout, lw, lh).0, level, "уровню {} — своя копия", level);
        }
        assert_eq!(
            pyramid::level_size(info.height, 1) - layout.overviews[0].height,
            0,
            "у чётной высоты расхождения нет — иначе допуск проверялся бы вхолостую"
        );
    }

    /// Нулевому уровню копий не бывает: он и есть родное разрешение, и
    /// прощать там нечего — округления не было.
    #[test]
    fn нулевой_уровень_читается_из_базового_ifd() {
        let (width, height) = (25437u32, 16729u32);
        let layout = halved_down(width, height, 5);
        let found = pick_source(&bare(width, height), &layout, width, height);
        assert_eq!(found, (0, width, height));

        // Вырожденный случай той же ловушки: у растра в два пикселя копия
        // ровно вдвое мельче, и абсолютный допуск пустил бы её под уровень 0.
        let tiny = Layout { tiled: true, overviews: vec![Overview { image: 1, width: 1, height: 1 }] };
        assert_eq!(pick_source(&bare(2, 2), &tiny, 2, 2), (0, 2, 2), "родному разрешению копий нет");
    }

    /// Допуск ровно в пиксель, а не «примерно». Копия у́же на два уже не
    /// годится: прощается округление, а не близость.
    #[test]
    fn допуск_ровно_в_один_пиксель() {
        let short = |w: u32| Layout {
            tiled: true,
            overviews: vec![Overview { image: 1, width: w, height: w }],
        };
        let info = bare(2000, 2000);
        assert_eq!(pick_source(&info, &short(499), 500, 500).0, 1, "у́же на пиксель — своя");
        assert_eq!(pick_source(&info, &short(498), 500, 500).0, 0, "у́же на два — чужая");
    }

    /// Стороны проверяются порознь: копия, годная по ширине, может не годиться
    /// по высоте. У вытянутого растра пиксель короткой стороны — это разы, и
    /// уровень собрался бы растягиванием вдвое.
    #[test]
    fn узкая_копия_не_годится_по_высоте() {
        let info = bare(513, 3);
        let layout = Layout { tiled: true, overviews: vec![Overview { image: 1, width: 256, height: 1 }] };
        let (lw, lh) = (pyramid::level_size(513, 1), pyramid::level_size(3, 1));
        assert_eq!((lw, lh), (257, 2));
        assert_eq!(
            pick_source(&info, &layout, lw, lh),
            (0, 513, 3),
            "копия высотой 1 под уровень высотой 2 не годится"
        );
    }

    /// Годных копий у уровня бывает несколько, и берётся самая мелкая — она
    /// дешевле всех по чтению. Порядок в файле при этом ничего не решает:
    /// копии перечислены в порядке обнаружения, а не по размеру.
    #[test]
    fn из_годных_копий_берётся_самая_мелкая() {
        let layout = Layout {
            tiled: true,
            overviews: vec![
                Overview { image: 3, width: 800, height: 800 },
                Overview { image: 1, width: 3200, height: 3200 },
                Overview { image: 2, width: 1600, height: 1600 },
            ],
        };
        let found = pick_source(&bare(6400, 6400), &layout, 800, 800);
        assert_eq!(found, (3, 800, 800), "годная мельче — та, что ровно под уровень");
    }

    /// Копия грубее уровня больше, чем на округление, не годится: тайл
    /// собрался бы растягиванием, а не ужатием.
    #[test]
    fn копия_грубее_уровня_не_годится() {
        let layout = Layout { tiled: true, overviews: vec![Overview { image: 1, width: 400, height: 400 }] };
        let found = pick_source(&bare(1000, 1000), &layout, 500, 500);
        assert_eq!(found, (0, 1000, 1000), "уровню в 500 копия в 400 не годится");
    }

    /// Заголовок каталога геоключей: версия, ревизия и число ключей.
    fn geokeys(model: u16) -> Vec<u16> {
        vec![1, 1, 0, 1, 1024, 0, 1, model]
    }

    /// Тот же каталог, но с GTRasterTypeGeoKey: 1 — координата названа для
    /// угла пикселя, 2 — для его середины.
    fn geokeys_raster(model: u16, raster: u16) -> Vec<u16> {
        vec![1, 1, 0, 2, 1024, 0, 1, model, 1025, 0, 1, raster]
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
        let ties = geo_ties(&geokeys(2), &points, &[], &[], 10572, 9993);
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
        let ties = geo_ties(&geokeys(2), &points, &[0.5, 0.25, 0.0], &[], 100, 200);
        assert_eq!(ties.len(), 4);
        assert_eq!((ties[0].lon, ties[0].lat), (10.0, 50.0));
        assert_eq!((ties[1].lon, ties[1].lat), (60.0, 50.0), "правый край восточнее");
        assert_eq!((ties[2].lon, ties[2].lat), (10.0, 0.0), "нижний край южнее");
    }

    /// Координата, названная для середины пикселя, сдвигает узел на его
    /// половину: наружу узел уезжает долей растра, где ноль — край. Так
    /// привязан Copernicus DSM, и половина его секундного шага — пятнадцать
    /// метров на местности.
    ///
    /// Сдвигается при этом весь растр, а не растягивается: размах между
    /// краями остаётся тем же.
    #[test]
    fn a_point_raster_shifts_the_node_by_half_a_pixel() {
        let points = vec![0.0, 0.0, 0.0, 13.0, 1.0, 0.0];
        let step = [1.0 / 3600.0, 1.0 / 3600.0, 0.0];
        let corner = geo_ties(&geokeys_raster(2, 1), &points, &step, &[], 3600, 3600);
        let middle = geo_ties(&geokeys_raster(2, 2), &points, &step, &[], 3600, 3600);

        assert_eq!((corner[0].lon, corner[0].lat), (13.0, 1.0), "угол назван как есть");
        let half = 0.5 / 3600.0;
        assert!((middle[0].lon - (13.0 - half)).abs() < 1e-12, "{}", middle[0].lon);
        assert!((middle[0].lat - (1.0 + half)).abs() < 1e-12, "{}", middle[0].lat);
        assert!(
            ((middle[1].lon - middle[0].lon) - (corner[1].lon - corner[0].lon)).abs() < 1e-12,
            "растр растянулся вместо сдвига по долготе"
        );
        assert!(
            ((middle[2].lat - middle[0].lat) - (corner[2].lat - corner[0].lat)).abs() < 1e-12,
            "растр растянулся вместо сдвига по широте"
        );
    }

    /// Один и тот же растр, названный парой «точка + шаг» и равной ей матрицей,
    /// ложится одинаково — и с серединой пикселя тоже. Ветки разные, и сдвиг в
    /// них берётся с разных концов: у пары — с начала отсчёта, у матрицы — с
    /// довода. Разойдись они, и один и тот же файл лежал бы по-разному в
    /// зависимости от того, каким тегом его привязали.
    #[test]
    fn the_matrix_and_the_step_place_a_point_raster_alike() {
        let (dx, dy) = (1.0 / 3600.0, 1.0 / 3600.0);
        let points = vec![0.0, 0.0, 0.0, 13.0, 1.0, 0.0];
        let matrix = vec![dx, 0.0, 0.0, 13.0, 0.0, -dy, 0.0, 1.0];
        let keys = geokeys_raster(2, 2);

        let by_step = geo_ties(&keys, &points, &[dx, dy, 0.0], &[], 3600, 3600);
        let by_matrix = geo_ties(&keys, &[], &[], &matrix, 3600, 3600);

        assert_eq!(by_step.len(), by_matrix.len());
        for (step, matrix) in by_step.iter().zip(&by_matrix) {
            assert!((step.lon - matrix.lon).abs() < 1e-12, "{} против {}", step.lon, matrix.lon);
            assert!((step.lat - matrix.lat).abs() < 1e-12, "{} против {}", step.lat, matrix.lat);
        }
        // И это не совпадение двух нулей: сдвиг в обеих ветках произошёл.
        let corner = geo_ties(&geokeys_raster(2, 1), &[], &[], &matrix, 3600, 3600);
        assert!((corner[0].lon - by_matrix[0].lon).abs() > dy / 4.0);
    }

    /// Решётка узлов сдвигается той же половиной пикселя: у неё пиксель назван
    /// теми же координатами, что и у остальных, и оставленная без сдвига она
    /// разошлась бы с ними на растре, привязанном обоими способами разом.
    #[test]
    fn a_tiepoint_lattice_shifts_by_half_a_pixel_too() {
        let points = vec![
            0.0, 0.0, 0.0, 2.707, 73.395, 0.0, //
            529.0, 0.0, 0.0, 2.096, 73.470, 0.0,
        ];
        let ties = geo_ties(&geokeys_raster(2, 2), &points, &[], &[], 10572, 9993);
        assert_eq!((ties[0].px, ties[0].py), (0.5, 0.5));
        assert_eq!((ties[1].px, ties[1].py), (529.5, 0.5));
        // Место узла при этом не трогают: сдвинулся пиксель, а не координата.
        assert_eq!((ties[0].lat, ties[0].lon), (73.395, 2.707));
    }

    /// Умолчание спецификации — угол: файл без GTRasterTypeGeoKey читается так
    /// же, как файл, объявивший угол прямо.
    #[test]
    fn a_raster_without_the_key_is_placed_by_its_corner() {
        let points = vec![0.0, 0.0, 0.0, 13.0, 1.0, 0.0];
        let step = [1.0 / 3600.0, 1.0 / 3600.0, 0.0];
        let silent = geo_ties(&geokeys(2), &points, &step, &[], 3600, 3600);
        let said = geo_ties(&geokeys_raster(2, 1), &points, &step, &[], 3600, 3600);
        assert_eq!((silent[0].lon, silent[0].lat), (said[0].lon, said[0].lat));
    }

    /// Копией считается уменьшенная картинка, но не уменьшенная маска: GDAL
    /// пишет копии маски как 5 = 1|4, и взятая за копию однобитная маска
    /// показалась бы вместо снимка.
    #[test]
    fn a_mask_is_not_an_overview() {
        assert!(is_overview(1), "уменьшенная копия");
        assert!(!is_overview(5), "уменьшенная копия маски");
        assert!(!is_overview(4), "маска в полный размер");
        assert!(!is_overview(0), "сам снимок");
        assert!(is_overview(3), "копия страницы многостраничного — всё ещё копия");
    }

    /// Повёрнутый растр привязан матрицей, и углы её слушаются: у повёрнутого
    /// верхний правый угол не на той же широте, что верхний левый, — а пара
    /// «точка + шаг» такого сказать не умеет вовсе.
    #[test]
    fn a_transformation_matrix_places_the_rotated_corners() {
        // Поворот на 90°: столбцы растра идут на восток, строки — на север.
        let matrix = vec![
            0.0, 0.5, 0.0, 10.0, //
            0.25, 0.0, 0.0, 50.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let ties = geo_ties(&geokeys(2), &[], &[], &matrix, 100, 200);
        assert_eq!(ties.len(), 4);
        assert_eq!((ties[0].lon, ties[0].lat), (10.0, 50.0));
        assert_eq!((ties[1].lon, ties[1].lat), (10.0, 75.0), "вправо по растру — на север");
        assert_eq!((ties[2].lon, ties[2].lat), (110.0, 50.0), "вниз по растру — на восток");
    }

    /// Матрица читается только там, где пары «точка + шаг» нет: спецификация
    /// разрешает ровно одну из двух привязок, и файл с обеими врёт хотя бы
    /// одной. Верить в таком случае надо той, которую понимают все.
    #[test]
    fn the_pair_outranks_the_matrix() {
        let points = vec![0.0, 0.0, 0.0, 10.0, 50.0, 0.0];
        let matrix = vec![
            0.0, 0.5, 0.0, 999.0, //
            0.25, 0.0, 0.0, 999.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let ties = geo_ties(&geokeys(2), &points, &[0.5, 0.25, 0.0], &matrix, 100, 200);
        assert_eq!((ties[0].lon, ties[0].lat), (10.0, 50.0));
    }

    /// Вырожденная матрица — не привязка: масштаба у неё нет, и все четыре
    /// угла сошлись бы в одну точку.
    #[test]
    fn a_degenerate_matrix_is_no_binding() {
        let flat = vec![0.0; 16];
        assert!(geo_ties(&geokeys(2), &[], &[], &flat, 100, 200).is_empty());
        // Как и обрезанная: восьми чисел на две строки не набралось.
        assert!(geo_ties(&geokeys(2), &[], &[], &[1.0, 0.0, 0.0], 100, 200).is_empty());
    }

    /// Привязка к проекции узлами в градусах не притворяется: перевести её мог
    /// бы только знающий саму проекцию, а тайлер про Землю не знает ничего.
    /// Уезжает она отдельным полем (см. [`geo_placement`]).
    #[test]
    fn projected_files_yield_nothing() {
        let points = vec![0.0, 0.0, 0.0, 600_000.0, 7_800_000.0, 0.0];
        assert!(geo_ties(&geokeys(1), &points, &[10.0, 10.0, 0.0], &[], 10, 10).is_empty());
        // Как и файл вовсе без геотегов.
        assert!(geo_ties(&[], &points, &[10.0, 10.0, 0.0], &[], 10, 10).is_empty());
    }

    /// Каталог ключей проекционного растра: модель — метры, плюс код системы.
    fn projected_keys(epsg: u16) -> Vec<u16> {
        vec![1, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, epsg]
    }

    /// Он же с GTRasterTypeGeoKey: 1 — координата названа для угла пикселя,
    /// 2 — для его середины.
    fn projected_keys_raster(epsg: u16, raster: u16) -> Vec<u16> {
        vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, raster, 3072, 0, 1, epsg]
    }

    /// Растр в проекции отдаётся кодом системы и преобразованием: пиксель (0,0)
    /// ложится ровно в опорную точку, шаг идёт по осям, а узлов в градусах у
    /// такого файла нет вовсе.
    ///
    /// Числа — с настоящего файла Landsat-5 (`LT51780121988065ESA00_B1.TIF`,
    /// зона 38 северная, шаг 30 м): ровно тот случай, ради которого поле и
    /// заведено.
    #[test]
    fn a_projected_raster_reports_its_system_and_transform() {
        let points = vec![0.0, 0.0, 0.0, 499_536.218_75, 7_693_329.5, 0.0];
        let found = geo_placement(&projected_keys(32638), &points, &[30.0, 30.0, 0.0], &[])
            .expect("привязка к зоне 38");

        assert_eq!(found.epsg, 32638);
        assert_eq!((found.affine[2], found.affine[5]), (499_536.218_75, 7_693_329.5));
        assert_eq!(found.affine[0], 30.0, "шаг на восток");
        // Шаг по Y положителен, а строки растра идут на юг.
        assert_eq!(found.affine[4], -30.0, "шаг на юг");
        assert_eq!((found.affine[1], found.affine[3]), (0.0, 0.0), "поворота у пары «точка+шаг» нет");

        assert!(
            geo_ties(&projected_keys(32638), &points, &[30.0, 30.0, 0.0], &[], 7956, 7740)
                .is_empty(),
            "метры зоны не должны уехать узлами в градусах"
        );
    }

    /// Опорная точка не обязана стоять в углу растра, а шаг — быть квадратным.
    /// На настоящем файле этого не видно: у Landsat опора в (0,0) и шаг 30×30, и
    /// на таких числах перепутанные оси неотличимы друг от друга.
    #[test]
    fn a_tiepoint_off_the_corner_places_the_whole_raster() {
        let points = vec![100.0, 200.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let found = geo_placement(&projected_keys(32638), &points, &[30.0, 15.0, 0.0], &[])
            .expect("привязка");

        assert_eq!(found.affine[0], 30.0, "шаг на восток");
        assert_eq!(found.affine[4], -15.0, "шаг на юг — свой, а не тот же");
        // Пиксель (0,0) стои́т на сто шагов западнее опоры и на двести севернее.
        assert_eq!(found.affine[2], 500_000.0 - 100.0 * 30.0);
        assert_eq!(found.affine[5], 7_000_000.0 + 200.0 * 15.0);
    }

    /// То же в градусной ветке: обе привязки называют опору одинаково, и
    /// разойтись в этом им нельзя — два растра одного снимка легли бы врозь.
    #[test]
    fn a_degree_tiepoint_off_the_corner_places_the_whole_raster() {
        let points = vec![100.0, 200.0, 0.0, 13.0, 50.0, 0.0];
        let ties = geo_ties(&geokeys(2), &points, &[0.5, 0.25, 0.0], &[], 10, 20);
        assert_eq!(ties[0].lon, 13.0 - 100.0 * 0.5, "сто шагов западнее опоры");
        assert_eq!(ties[0].lat, 50.0 + 200.0 * 0.25, "двести шагов севернее");
    }

    /// Полпикселя у повёрнутой матрицы снимается по обеим осям сразу: у растра
    /// по осям второй член нулевой, и выброшенный, он там не виден вовсе.
    #[test]
    fn the_half_pixel_of_a_rotated_matrix_takes_both_axes() {
        let matrix = vec![
            0.0, 30.0, 0.0, 500_000.0, //
            30.0, 0.0, 0.0, 7_000_000.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let found =
            geo_placement(&projected_keys_raster(32638, 2), &[], &[], &matrix).expect("поворот");
        assert_eq!(found.affine[2], 500_000.0 - 15.0, "полшага по второму члену");
        assert_eq!(found.affine[5], 7_000_000.0 - 15.0, "и по нему же в другой строке");
    }

    /// Матрица со сложенными строками вырождена, хотя ни один её член не ноль:
    /// растр складывается в линию. Держится это знаком в определителе — при
    /// сложении вместо вычитания такая привязка проехала бы насквозь.
    #[test]
    fn a_shear_that_collapses_the_raster_is_no_binding() {
        let shear = vec![
            30.0, 30.0, 0.0, 500_000.0, //
            30.0, 30.0, 0.0, 7_000_000.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        assert!(geo_placement(&projected_keys(32638), &[], &[], &shear).is_none());
        assert!(geo_ties(&geokeys(2), &[], &[], &shear, 10, 10).is_empty());
    }

    /// Не-число привязкой не является ни в одной ветке и ни в одном теге.
    /// Сравнение с нулём его не ловит — `NaN != 0` истинно, — а доехав до рамки,
    /// оно даёт слой, который не выберет уровень пирамиды никогда.
    #[test]
    fn a_transform_of_not_a_number_binds_nothing() {
        let points = vec![0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let sick_step = [f64::NAN, 30.0, 0.0];
        assert!(geo_placement(&projected_keys(32638), &points, &sick_step, &[]).is_none(), "шаг");
        assert!(geo_ties(&geokeys(2), &points, &sick_step, &[], 10, 10).is_empty(), "он же в градусах");

        // Шаг годен, а место названо не числом: рамка вышла бы целой с виду.
        let sick_tie = vec![0.0, 0.0, 0.0, f64::NAN, 7_000_000.0, 0.0];
        assert!(
            geo_placement(&projected_keys(32638), &sick_tie, &[30.0, 30.0, 0.0], &[]).is_none(),
            "опорная точка"
        );

        let sick_matrix = vec![
            30.0, 0.0, 0.0, f64::NAN, //
            0.0, -30.0, 0.0, 7_000_000.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        assert!(geo_placement(&projected_keys(32638), &[], &[], &sick_matrix).is_none(), "матрица");
    }

    /// Градусный файл проекцией не притворяется — и наоборот. Ветки
    /// взаимоисключимы: одна из них обязана промолчать на числах другой, иначе
    /// метры зоны уехали бы широтой.
    #[test]
    fn the_two_bindings_do_not_answer_for_each_other() {
        let points = vec![0.0, 0.0, 0.0, 13.0, 1.0, 0.0];
        let step = [1.0 / 3600.0, 1.0 / 3600.0, 0.0];

        assert!(geo_placement(&geokeys(2), &points, &step, &[]).is_none(), "градусы — не проекция");
        assert!(!geo_ties(&geokeys(2), &points, &step, &[], 3600, 3600).is_empty());

        let metres = vec![0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let keys = projected_keys(32638);
        assert!(geo_ties(&keys, &metres, &[30.0, 30.0, 0.0], &[], 10, 10).is_empty(),
            "проекция — не градусы");
        assert!(geo_placement(&keys, &metres, &[30.0, 30.0, 0.0], &[]).is_some());
    }

    /// Полпикселя середины снимается у проекции так же, как у градусной ветки:
    /// начало преобразования уезжает на полшага в ту же сторону. Разойдись эти
    /// две конвенции — и два растра одного снимка легли бы со сдвигом друг
    /// относительно друга.
    #[test]
    fn the_half_pixel_moves_both_bindings_the_same_way() {
        let points = vec![0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let step = [30.0, 30.0, 0.0];

        let corner = geo_placement(&projected_keys_raster(32638, 1), &points, &step, &[])
            .expect("угол пикселя");
        let middle = geo_placement(&projected_keys_raster(32638, 2), &points, &step, &[])
            .expect("середина пикселя");

        assert_eq!((corner.affine[2], corner.affine[5]), (500_000.0, 7_000_000.0));
        assert_eq!(middle.affine[2] - corner.affine[2], -15.0, "на полшага на запад");
        assert_eq!(middle.affine[5] - corner.affine[5], 15.0, "на полшага на север");

        // Та же пара для градусной ветки: знаки обязаны совпасть.
        let degrees = [1.0 / 3600.0, 1.0 / 3600.0, 0.0];
        let node = vec![0.0, 0.0, 0.0, 13.0, 1.0, 0.0];
        let by_corner = geo_ties(&geokeys_raster(2, 1), &node, &degrees, &[], 10, 10);
        let by_middle = geo_ties(&geokeys_raster(2, 2), &node, &degrees, &[], 10, 10);
        assert!(by_middle[0].lon < by_corner[0].lon, "на полшага на запад");
        assert!(by_middle[0].lat > by_corner[0].lat, "на полшага на север");
    }

    /// Матрица и пара «точка + шаг» задают одно преобразование и обязаны дать
    /// одинаковое аффинное — на одних и тех же числах. Порознь эти две ветки
    /// разошлись бы знаком по Y и никто бы этого не заметил.
    #[test]
    fn the_matrix_and_the_step_describe_one_projection() {
        let step = 30.0;
        let points = vec![0.0, 0.0, 0.0, 499_536.0, 7_693_329.0, 0.0];
        let matrix = vec![
            step, 0.0, 0.0, 499_536.0, //
            0.0, -step, 0.0, 7_693_329.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let keys = projected_keys_raster(32638, 2);
        let by_step = geo_placement(&keys, &points, &[step, step, 0.0], &[]).expect("шагом");
        let by_matrix = geo_placement(&keys, &[], &[], &matrix).expect("матрицей");
        assert_eq!(by_step.affine, by_matrix.affine);
    }

    /// Повёрнутая матрица доезжает поворотом, а не своей диагональю: у растра,
    /// снятого не по осям зоны, внедиагональные члены и есть вся привязка.
    #[test]
    fn a_rotated_matrix_keeps_its_rotation() {
        // Поворот на 90°: восток растра идёт на север зоны.
        let matrix = vec![
            0.0, 30.0, 0.0, 500_000.0, //
            30.0, 0.0, 0.0, 7_000_000.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let found = geo_placement(&projected_keys(32638), &[], &[], &matrix).expect("поворот");
        assert_eq!((found.affine[0], found.affine[1]), (0.0, 30.0));
        assert_eq!((found.affine[3], found.affine[4]), (30.0, 0.0));
    }

    /// Система, названная непонятно, — это не система. Ноль и user-defined
    /// кодом не являются: параметры такой проекции лежат отдельными ключами, и
    /// собрать её из них значило бы разбирать проекции, а не читать растр.
    #[test]
    fn a_system_without_a_code_is_no_system() {
        let points = vec![0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let step = [30.0, 30.0, 0.0];
        let bare = vec![1, 1, 0, 1, 1024, 0, 1, 1];

        assert!(geo_placement(&bare, &points, &step, &[]).is_none(), "кода нет вовсе");
        assert!(geo_placement(&projected_keys(32767), &points, &step, &[]).is_none(), "user-defined");
        assert!(geo_placement(&projected_keys(0), &points, &step, &[]).is_none(), "ноль");
    }

    /// Решётка опорных точек в метрах зоны — это отказ, а не привязка по
    /// первому узлу: решётка описывает нелинейную раскладку, и шестёркой чисел
    /// она не выражается. Взятая по одному узлу, она врала бы тем сильнее, чем
    /// дальше от него.
    #[test]
    fn a_projected_lattice_is_refused_not_flattened() {
        let points = vec![
            0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0, //
            100.0, 0.0, 0.0, 503_000.0, 7_000_100.0, 0.0,
        ];
        assert!(geo_placement(&projected_keys(32638), &points, &[30.0, 30.0, 0.0], &[]).is_none());
    }

    /// Вырожденное преобразование привязкой не является — ни в одной ветке и ни
    /// в одной модели координат: растр сложился бы в линию или в точку, а
    /// снаружи такая рамка неотличима от настоящей.
    #[test]
    fn a_degenerate_transform_binds_nothing() {
        let points = vec![0.0, 0.0, 0.0, 500_000.0, 7_000_000.0, 0.0];
        let flat = vec![
            30.0, 0.0, 0.0, 500_000.0, //
            0.0, 0.0, 0.0, 7_000_000.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        for step in [[0.0, 30.0, 0.0], [30.0, 0.0, 0.0]] {
            assert!(geo_placement(&projected_keys(32638), &points, &step, &[]).is_none(), "шаг");
            assert!(
                geo_ties(&geokeys(2), &points, &step, &[], 10, 10).is_empty(),
                "тот же нулевой шаг в градусах"
            );
        }
        assert!(geo_placement(&projected_keys(32638), &[], &[], &flat).is_none(), "матрица");
        assert!(geo_ties(&geokeys(2), &[], &[], &flat, 10, 10).is_empty(), "она же в градусах");
    }

    /// Чужой датум называется вслух: места для него дальше по течению нет — и
    /// узлы, и рамка объявлены в WGS84, — так что файл на Пулково-42 уехал бы
    /// туда молча, а это сотня метров в средней полосе.
    ///
    /// Молчание файла и user-defined чужим датумом не являются: сказать о них
    /// нечего, и строка о них была бы шумом в каждом логе.
    #[test]
    fn a_foreign_datum_is_named_aloud() {
        let with = |code: u16| vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, code];
        assert_eq!(foreign_datum(&with(4284)), Some(4284), "Пулково-42");
        assert_eq!(foreign_datum(&with(4258)), Some(4258), "ETRS89");
        assert_eq!(foreign_datum(&with(4326)), None, "он и есть WGS84");
        assert_eq!(foreign_datum(&with(4979)), None, "он же, трёхмерный");
        assert_eq!(foreign_datum(&with(32767)), None, "user-defined — не ответ");
        assert_eq!(foreign_datum(&geokeys(2)), None, "ключа нет вовсе");
    }

    /// Цветовая модель разбирается один раз и отвечает обоим спрашивающим — и
    /// о раскладке пикселя, и о разрядности. Многополосный без цветовой
    /// интерпретации — это стопка величин: показывается первая, а вторая не
    /// прозрачность, хотя сэмплов у неё, как у серого с альфой, ровно два.
    #[test]
    fn colour_model_answers_channels_and_depth() {
        assert_eq!(model(tiff::ColorType::Gray(16)).unwrap().1, 16);
        assert_eq!(model(tiff::ColorType::RGBA(8)).unwrap().0.channels, 4);
        assert_eq!(pixel(tiff::ColorType::RGB(8)).unwrap().channels, 3);
        let (stack, bits) = model(tiff::ColorType::Multiband { bit_depth: 8, num_samples: 2 })
            .expect("стопка величин — не отказ");
        assert_eq!((stack.channels, stack.colors(), stack.has_alpha(), bits), (2, 1, false, 8));
        assert!(pixel(tiff::ColorType::CMYK(8)).is_err());
    }

    /// Обрезанный чанк кончается строкой, а не выходом за границу среза.
    /// Разница здесь не стилистическая: срез уронил бы инстанс трапом, после
    /// которого заказчик ждёт ответа, которого никто не пришлёт.
    #[test]
    fn a_short_chunk_is_refused_not_blitted() {
        let mapping = Mapping::identity(None);
        let whole = DecodingResult::U8(vec![7u8; 4 * 4]);
        let rgba = chunk_rgba(&mapping, &whole, Pixel::named(1), 4, 4).expect("целый чанк");
        assert_eq!(rgba.len(), 4 * 4 * 4);

        let short = DecodingResult::U8(vec![7u8; 4 * 3]);
        let refused = chunk_rgba(&mapping, &short, Pixel::named(1), 4, 4).expect_err("чанк неполон");
        assert!(refused.contains("неполным"), "{}", refused);
    }
}
