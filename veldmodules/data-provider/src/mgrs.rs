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
/// Высота широтной полосы, градусы.
const BAND_DEG: f64 = 8.0;
/// Высота полосы X — единственной, что шире прочих: ею закрыта Арктика до 84°.
const LAST_BAND_DEG: f64 = 12.0;

/// Буквы широтных полос, с юга на север по 8°; I и O пропущены.
const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";
/// Буквы рядов стоклиц, цикл из двадцати; I и O пропущены.
const ROWS: &[u8] = b"ABCDEFGHJKLMNPQRSTUV";

/// Середина широтной полосы по её номеру, градусы.
///
/// Полосы идут по 8° от −80°, и только последняя, X, — двенадцатиградусная: ею
/// закрыли Арктику до 84°, не заводя ещё одной буквы. Считай её середину по
/// общему правилу — и окно выбора ряда съедет на два градуса к югу: северному
/// краю полосы останется 112 км запаса вместо 354. Этого пока хватает, но
/// запас, ужавшийся втрое против остальных полос, держится уже ни на чём
/// (см. тест).
fn band_middle_deg(band: usize) -> f64 {
    let south_edge = -80.0 + band as f64 * BAND_DEG;
    let height = match band == BANDS.len() - 1 {
        true => LAST_BAND_DEG,
        false => BAND_DEG,
    };
    south_edge + height / 2.0
}

/// Код тайла из имени продукта: сегмент `_TxxABC_` с зоной и тремя буквами.
///
/// Имя приходит из хранилища, поэтому длина проверяется до всякого среза и
/// в одном условии с ними: на `T` начинается не только код гранулы, но и,
/// например, тир Landsat (`..._T1`), а срез мимо длины — паника, а не отказ.
/// Режется байтами: пятизначный код латиницей, и граница символа у среза по
/// байтам не спрашивается.
pub fn tile_of(product_name: &str) -> Option<&str> {
    product_name.split('_').find_map(|part| {
        let tile = part.strip_prefix('T')?;
        let bytes = tile.as_bytes();
        let ok = bytes.len() == 5
            && bytes[..2].iter().all(u8::is_ascii_digit)
            && bytes[2..].iter().all(u8::is_ascii_uppercase);
        ok.then_some(tile)
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
    // грубого метра на градус хватает — от середины до края полосы 444 км
    // (у полосы X 666), то есть до половины периода остаётся втрое больше
    // пройденного, а ошибка линейного приближения (до 12 км у 84-й параллели)
    // в этот запас укладывается с избытком.
    let row = ROWS
        .iter()
        .position(|&b| b == bytes[4])
        .ok_or_else(|| format!("mgrs: ряда '{}' не бывает", bytes[4] as char))?;
    let shift = if zone % 2 == 0 { 5 } else { 0 };
    let base = ((row + ROWS.len() - shift) % ROWS.len()) as f64 * SQUARE_M;

    let band_mid_deg = band_middle_deg(band);
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

    /// Окно выбора ряда обязано накрывать полосу с запасом с обеих сторон:
    /// буква ряда называет нортинг с точностью до периода в 2000 км, и
    /// разрешает её середина полосы. Полоса X двенадцатиградусная, и посчитай
    /// мы её середину общим восьмиградусным правилом — окно съехало бы на два
    /// градуса к югу, оставив северному краю 112 км запаса вместо 354.
    #[test]
    fn every_band_fits_the_row_window_from_both_sides() {
        for band in 0..BANDS.len() {
            let height = if band == BANDS.len() - 1 { LAST_BAND_DEG } else { BAND_DEG };
            let south = -80.0 + band as f64 * BAND_DEG;
            let target = band_middle_deg(band).abs() * 110_946.0;

            for (edge, side) in [(south, "юг"), (south + height, "север")] {
                let margin = ROW_CYCLE_M / 2.0 - (edge.abs() * 110_946.0 - target).abs();
                assert!(
                    margin > 250_000.0,
                    "полоса {} с {}а: запас {:.0} м",
                    BANDS[band] as char, side, margin
                );
            }
        }
    }

    /// Полоса X шире прочих, и это записано в самом правиле, а не подогнано
    /// числом: её середина 78°, а не 76°.
    #[test]
    fn the_x_band_is_twelve_degrees_wide() {
        assert_eq!(band_middle_deg(BANDS.len() - 1), 78.0);
        assert_eq!(band_middle_deg(0), -76.0, "полоса C — обычная, от −80° до −72°");
    }

    #[test]
    fn tile_is_read_from_product_name() {
        assert_eq!(
            tile_of("S2C_MSIL2A_20260812T081601_N0512_R121_T40WFC_20260812T121315.SAFE"),
            Some("40WFC")
        );
        assert_eq!(tile_of("S1C_IW_GRDH_1SDV_20260810T031955_011C6E_EDC8.SAFE"), None);
    }

    /// Имя приходит из хранилища, и на `T` там начинается не только гранула.
    /// Короткий сегмент — отказ, а не срез мимо длины: тир Landsat (`_T1`)
    /// стоит в имени каждого его продукта.
    #[test]
    fn short_and_odd_segments_are_refused_not_sliced() {
        assert_eq!(tile_of("LC09_L2SP_02_T1"), None);
        assert_eq!(tile_of("A_T_B"), None);
        assert_eq!(tile_of("T"), None);
        // Пять знаков, но не пять байт: срез по байтам границы символа не
        // спрашивает, а код гранулы латиницей.
        assert_eq!(tile_of("T40WФC"), None);
        assert_eq!(tile_of("T40wfc"), None);
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
