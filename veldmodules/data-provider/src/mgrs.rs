//! Сетка MGRS: код тайла Sentinel-2 → рамка гранулы в метрах её зоны UTM.
//!
//! Гранула S2 — квадрат 109 800 м, чей верхний левый угол стоит в углу
//! стоклицы MGRS (юго-западный угол квадрата 100×100 км плюс 100 км к северу);
//! перекрытие соседних гранул — следствие того, что углы идут шагом 100 км, а
//! квадрат шире. Ни каталог, ни листинг этих чисел не сообщают — они зашиты в
//! само имя тайла (`T40WFC`), и разбирает его тот, кто знает раскладку
//! продуктов, то есть этот модуль. Проекционной математики здесь нет: рамка
//! отдаётся числами зоны, переводит их в градусы потребитель (глобус).
//!
//! Буквенная арифметика — по определению сетки: столбцы идут восьмёрками
//! A–H / J–R / S–Z по циклу из трёх зон, ряды — двадцаткой A–V (без I и O) с
//! половинным сдвигом в чётных зонах, и повторяются каждые 2 000 000 м.
//! Повтор разрешается широтной полосой из того же кода тайла.

/// Рамка растра в его зоне UTM. `y1` — северный край: строки растра идут с
/// севера на юг, как их кладут файлы гранул.
pub struct Frame {
    pub zone: u32,
    pub south: bool,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

/// Сторона гранулы Sentinel-2, метры: 10980 пикселей по 10 м.
const TILE_SIDE_M: f64 = 109_800.0;
/// Сторона стоклицы MGRS.
const SQUARE_M: f64 = 100_000.0;
/// Период повторения буквы ряда.
const ROW_CYCLE_M: f64 = 2_000_000.0;

/// Буквы широтных полос, с юга на север по 8°; I и O пропущены.
const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";
/// Буквы рядов стоклиц, цикл из двадцати; I и O пропущены.
const ROWS: &[u8] = b"ABCDEFGHJKLMNPQRSTUV";

/// Код тайла из имени продукта: сегмент `_TxxABC_` с зоной и тремя буквами.
pub fn tile_of(product_name: &str) -> Option<&str> {
    product_name.split('_').find_map(|part| {
        let tile = part.strip_prefix('T')?;
        let zone_ok = tile.len() == 5 && tile[..2].bytes().all(|b| b.is_ascii_digit());
        let letters_ok = tile[2..].bytes().all(|b| b.is_ascii_uppercase());
        (zone_ok && letters_ok).then_some(tile)
    })
}

/// Рамка гранулы по коду тайла (`40WFC`).
pub fn frame(tile: &str) -> Result<Frame, String> {
    let bytes = tile.as_bytes();
    if bytes.len() != 5 {
        return Err(format!("mgrs: код тайла '{}' не из пяти знаков", tile));
    }
    let zone: u32 = tile[..2].parse().map_err(|_| format!("mgrs: зона в '{}' не число", tile))?;
    if !(1..=60).contains(&zone) {
        return Err(format!("mgrs: зоны {} не бывает", zone));
    }
    let band = BANDS
        .iter()
        .position(|&b| b == bytes[2])
        .ok_or_else(|| format!("mgrs: широтной полосы '{}' не бывает", bytes[2] as char))?;

    // Столбец: три набора по восемь букв, зона выбирает набор.
    let column_set: &[u8] = match (zone - 1) % 3 {
        0 => b"ABCDEFGH",
        1 => b"JKLMNPQR",
        _ => b"STUVWXYZ",
    };
    let column = column_set
        .iter()
        .position(|&b| b == bytes[3])
        .ok_or_else(|| format!("mgrs: столбца '{}' нет в зоне {}", bytes[3] as char, zone))?;
    let x0 = (column as f64 + 1.0) * SQUARE_M;

    // Ряд: буква даёт нортинг с точностью до периода 2000 км; чётные зоны
    // сдвинуты на пять букв. Период разрешается серединой широтной полосы:
    // грубого метра на градус хватает с запасом — ошибка приближения на
    // порядки меньше половины периода.
    let row = ROWS
        .iter()
        .position(|&b| b == bytes[4])
        .ok_or_else(|| format!("mgrs: ряда '{}' не бывает", bytes[4] as char))?;
    let shift = if zone % 2 == 0 { 5 } else { 0 };
    let base = ((row + ROWS.len() - shift) % ROWS.len()) as f64 * SQUARE_M;

    let band_mid_deg = -80.0 + band as f64 * 8.0 + 4.0;
    let south = band_mid_deg < 0.0;
    let approx = band_mid_deg.abs() * 110_946.0;
    // Кандидаты нортинга южного края стоклицы: от экватора у северных полос,
    // от 10 000 000 вниз у южных (ложный сдвиг южных зон).
    let target = if south { 10_000_000.0 - approx } else { approx };
    let mut northing = base;
    while northing + ROW_CYCLE_M / 2.0 < target {
        northing += ROW_CYCLE_M;
    }

    let y1 = northing + SQUARE_M;
    Ok(Frame { zone, south, x0, y0: y1 - TILE_SIDE_M, x1: x0 + TILE_SIDE_M, y1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_is_read_from_product_name() {
        assert_eq!(
            tile_of("S2C_MSIL2A_20260812T081601_N0512_R121_T40WFC_20260812T121315.SAFE"),
            Some("40WFC")
        );
        assert_eq!(tile_of("S1C_IW_GRDH_1SDV_20260810T031955_011C6E_EDC8.SAFE"), None);
    }

    /// Опорная гранула T40WFC: числа сверены с её MTD_TL.xml
    /// (EPSG:32640, ULX 600000, ULY 7800000, 10980 пикселей по 10 м).
    #[test]
    fn reference_granule_frame_matches_its_xml() {
        let frame = frame("40WFC").unwrap();
        assert_eq!((frame.zone, frame.south), (40, false));
        assert_eq!((frame.x0, frame.y1), (600_000.0, 7_800_000.0));
        assert_eq!((frame.x1 - frame.x0, frame.y1 - frame.y0), (109_800.0, 109_800.0));
    }

    /// Нечётная зона без сдвига рядов: у экваториального тайла зоны 31 ряд
    /// с буквы A. T31NAA — юго-западный угол зоны на экваторе.
    #[test]
    fn odd_zone_rows_start_at_a() {
        let frame = frame("31NAA").unwrap();
        assert_eq!((frame.x0, frame.y1), (100_000.0, 100_000.0));
        assert!(!frame.south);
    }

    /// Южное полушарие: нортинг отсчитан вниз от ложного сдвига 10 000 000 и
    /// остаётся в его пределах.
    #[test]
    fn southern_band_stays_under_false_northing() {
        let frame = frame("33HVB").unwrap();
        assert!(frame.south);
        assert!(frame.y1 < 10_000_000.0 && frame.y0 > 5_000_000.0, "{}..{}", frame.y0, frame.y1);
    }

    #[test]
    fn garbage_codes_are_refused() {
        assert!(frame("40WIC").is_err()); // I в столбцах не живёт
        assert!(frame("00WFC").is_err());
        assert!(frame("4WFC").is_err());
        assert!(frame("40ZFC").is_err()); // полосы Z нет
    }
}
