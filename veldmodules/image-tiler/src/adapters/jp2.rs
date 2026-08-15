//! JPEG 2000 (JP2/J2C): квиклуки и гранулы Sentinel-2.
//!
//! Декодер (hayro-jpeg2000) читает кадр целиком и умеет декодировать сразу в
//! огрублённое разрешение: пропуск уровней DWT — та же ceil-лестница деления
//! пополам, что у пирамиды, поэтому декод всегда ложится на её уровень.
//! Но огрубление — лишь пожелание: палитровым файлам крейт молча его
//! выключает, пропуск не бывает глубже размеченного в файле числа уровней
//! DWT, а floor-математика выбора пропуска на нечётных размерах отстаёт на
//! ступень — декод выходит запрошенным уровнем или мельче. Каскад стартует с
//! фактического уровня: запрошенные тайлы производятся по пути, а уровни
//! мельче бонусом уезжают в кэш.
//!
//! Регионального декода у крейта нет, отсюда две границы. Память прохода
//! растёт с площадью декодируемого кадра, поэтому бюджет проверяется дважды:
//! по запрошенному уровню до чтения файла и по фактическим размерам декода
//! (`Image::width/height`, уже с учётом пропуска) до самого декода; не
//! влезает — честный отказ (у TCI 10980² это родное разрешение и половина;
//! их закроет регион в свою фазу). А описание источника не декодирует ничего:
//! размеры читаются своим разбором заголовка (`header_dims`) из первых
//! килобайт — тянуть гигабайтный файл ради describe нельзя.

use std::io::Read;

use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid;
use super::{to_rgba, Info, Kind, Metered};

/// Потолок памяти одного прохода: сам файл, f32-плоскости декодера, его
/// интерливленный выход и RGBA. Считается до чтения; уровень, не влезающий в
/// потолок, — отказ, а не смерть инстанса на пределе в 1 ГБ.
const DECODE_BUDGET: u64 = 512 * 1024 * 1024;

/// Сколько головы файла хватает заголовку: сигнатурные боксы, ftyp, jp2h
/// лежат первыми, а у сырого кодстрима SIZ — сразу за SOC.
const HEAD: usize = 64 * 1024;

pub fn describe(mut reader: Metered, len: u64) -> Result<Info, String> {
    let mut head = vec![0u8; HEAD.min(len as usize)];
    reader.read_exact(&mut head).map_err(|e| format!("jp2: чтение заголовка: {}", e))?;
    let (width, height) = header_dims(&head)?;
    let mut info = Info::plain(width, height, Kind::Jp2);
    info.finest = finest(len, width, height);
    Ok(info)
}

/// Самый подробный уровень, который влезает в бюджет декода. Считается по тем
/// же [`estimate`] и [`DECODE_BUDGET`], которыми [`produce`] потом и отказывает,
/// — затем и считается заранее: заказчику не за чем просить то, что заведомо
/// получит отказ (см. `Described.finest`).
///
/// Уровней у пирамиды конечное число, и если не влезает ни один — отдаётся
/// последний: вершина помещается в тайл, и не влезть она может только вместе с
/// самим файлом, который в память всё равно поднимать.
fn finest(len: u64, width: u32, height: u32) -> u32 {
    let top = pyramid::level_count(width, height).saturating_sub(1);
    (0..=top)
        .find(|level| {
            let (w, h) = (pyramid::level_size(width, *level), pyramid::level_size(height, *level));
            estimate(len, w, h) <= DECODE_BUDGET
        })
        .unwrap_or(top)
}

pub fn produce(
    mut reader: Metered,
    len: u64,
    info: &Info,
    level: u32,
    emit: Emit,
) -> Result<(), String> {
    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);
    // Быстрый отказ до чтения файла: запрошенный уровень — нижняя граница
    // памяти декода.
    let cost = estimate(len, lw, lh);
    if cost > DECODE_BUDGET {
        return Err(format!(
            "jp2: уровень {}×{} не влезает в бюджет декодера (≈{} МиБ) — возьмите уровень грубее",
            lw, lh, cost >> 20
        ));
    }

    // Файл целиком: декодеру нужен непрерывный срез. Через Metered — чтение
    // и есть та часть прохода, чей ход стоит показывать.
    let mut data = Vec::with_capacity(len as usize);
    reader.read_to_end(&mut data).map_err(|e| format!("jp2: чтение: {}", e))?;

    let settings = DecodeSettings {
        target_resolution: Some((lw, lh)),
        ..DecodeSettings::default()
    };
    let image = Image::new(&data, &settings).map_err(|e| format!("jp2: {:?}", e))?;
    match image.color_space() {
        ColorSpace::Gray | ColorSpace::RGB => {}
        // ICC-профиль не применяется — для превью числа берутся как есть.
        ColorSpace::Icc { .. } => {}
        other => return Err(format!("jp2: цветовая модель {:?} не поддерживается", other)),
    }

    // Фактические размеры декода известны только теперь (см. заголовок файла:
    // огрубление — пожелание) — бюджет пересчитывается до самого декода.
    let (dw, dh) = (image.width(), image.height());
    let cost = estimate(len, dw, dh);
    if cost > DECODE_BUDGET {
        return Err(format!(
            "jp2: файл не даёт декодировать грубее {}×{}, а это не влезает в бюджет (≈{} МиБ)",
            dw, dh, cost >> 20
        ));
    }
    let base = ladder_level(info.width, info.height, dw, dh, level).ok_or_else(|| {
        format!("jp2: декод {}×{} мимо лестницы пирамиды {}×{}", dw, dh, info.width, info.height)
    })?;

    let mut ctx = DecoderContext::default();
    let decoded = image.decode(&mut ctx).map_err(|e| format!("jp2: {:?}", e))?;
    let components = decoded.components();
    let channels = components.len();
    if channels == 0 || channels > 4 {
        return Err(format!("jp2: {} компонентов не разложить в RGBA", channels));
    }
    if components.iter().any(|c| c.samples().len() != components[0].samples().len()) {
        return Err("jp2: субдискретизация компонентов не поддерживается".to_string());
    }
    if components[0].samples().len() != (dw as usize) * (dh as usize) {
        return Err(format!(
            "jp2: декод отдал {} сэмплов вместо {}×{}",
            components[0].samples().len(),
            dw,
            dh
        ));
    }

    let samples = decoded.data_u8();
    drop(data);

    let mut rgba = to_rgba(&samples, channels, (dw as usize) * (dh as usize));
    drop(samples);

    // У файлов без собственной альфы поля гранулы приезжают нулём: у
    // Sentinel-2 ноль зарезервирован под «нет данных» (валидные значения —
    // 1..255). Прозрачность здесь, а не у потребителей: тайл — готовый RGBA.
    if matches!(channels, 1 | 3) {
        key_margins(&mut rgba, dw, dh);
    }

    let mut cascade = Cascade::new(base, dw, dh);
    cascade.push_rows(&rgba, dh, emit)?;
    cascade.finish(emit)
}

/// Прозрачность полей: заливка от краёв кадра по нулевым пикселям. Выбить
/// весь чёрный нельзя — настоящему чёрному в произвольном JP2 никто не
/// запрещал быть, — а поле гранулы от него отличается ровно тем, что
/// примыкает к краю. Внутренние нули остаются цветом.
fn key_margins(rgba: &mut [u8], w: u32, h: u32) {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return;
    }
    let zero = |rgba: &[u8], px: usize| rgba[px * 4..px * 4 + 3] == [0, 0, 0];

    let mut seen = vec![false; w * h];
    let mut queue: Vec<usize> = Vec::new();
    let push = |rgba: &[u8], seen: &mut [bool], queue: &mut Vec<usize>, px: usize| {
        if !seen[px] && zero(rgba, px) {
            seen[px] = true;
            queue.push(px);
        }
    };

    for x in 0..w {
        push(rgba, &mut seen, &mut queue, x);
        push(rgba, &mut seen, &mut queue, (h - 1) * w + x);
    }
    for y in 0..h {
        push(rgba, &mut seen, &mut queue, y * w);
        push(rgba, &mut seen, &mut queue, y * w + w - 1);
    }

    while let Some(px) = queue.pop() {
        rgba[px * 4 + 3] = 0;
        let (x, y) = (px % w, px / w);
        if x > 0 {
            push(rgba, &mut seen, &mut queue, px - 1);
        }
        if x + 1 < w {
            push(rgba, &mut seen, &mut queue, px + 1);
        }
        if y > 0 {
            push(rgba, &mut seen, &mut queue, px - w);
        }
        if y + 1 < h {
            push(rgba, &mut seen, &mut queue, px + w);
        }
    }
}

/// Пик памяти прохода при декоде в w×h: сам файл, f32-плоскости декодера
/// (до четырёх каналов), его интерливленный u8-выход и RGBA.
fn estimate(file_len: u64, w: u32, h: u32) -> u64 {
    file_len + u64::from(w) * u64::from(h) * (4 * 4 + 4 + 4)
}

/// Уровень пирамиды с такими размерами, от нулевого до `up_to`. Декод обязан
/// лежать на ceil-лестнице пирамиды — размеры мимо неё значат, что декодер
/// считает пропуск иначе, и доверять его выходу нельзя.
fn ladder_level(width: u32, height: u32, dw: u32, dh: u32, up_to: u32) -> Option<u32> {
    (0..=up_to)
        .find(|k| (pyramid::level_size(width, *k), pyramid::level_size(height, *k)) == (dw, dh))
}

// ── Разбор заголовка без декодера ──────────────────────────────

/// Сигнатура контейнера JP2 (signature box) и сырого кодстрима (SOC+SIZ).
pub const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
pub const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";

/// Размеры растра из головы файла: у контейнера — box `jp2h`→`ihdr`, у сырого
/// кодстрима — маркер SIZ. Ошибка — заголовок битый либо не поместился в
/// прочитанную голову.
pub fn header_dims(head: &[u8]) -> Result<(u32, u32), String> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return siz_dims(&head[2..]);
    }
    if !head.starts_with(JP2_MAGIC) {
        return Err("jp2: нет сигнатуры".to_string());
    }

    // Боксы верхнего уровня: [длина u32][тип 4Б][тело]. Длина 1 — расширенная
    // (следующие 8 байт), 0 — до конца файла.
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
                (at + 16, at.saturating_add(xlen as usize))
            }
            _ => (at + 8, at.saturating_add(len as usize)),
        };
        if kind == b"jp2h" {
            // Внутри супербокса ihdr обязан идти первым (ISO 15444-1, I.5.3):
            // [длина][ihdr][высота u32][ширина u32][каналы u16]…
            let ihdr = body;
            if ihdr + 16 > head.len() || &head[ihdr + 4..ihdr + 8] != b"ihdr" {
                return Err("jp2: jp2h без ihdr первым боксом".to_string());
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
    Err("jp2: заголовок не найден в голове файла".to_string())
}

/// SIZ: [маркер FF51][Lsiz u16][Rsiz u16][Xsiz u32][Ysiz u32][XOsiz][YOsiz]…
/// Размер растра — область холста за вычетом смещения.
fn siz_dims(from_siz: &[u8]) -> Result<(u32, u32), String> {
    if from_siz.len() < 22 || from_siz[0] != 0xFF || from_siz[1] != 0x51 {
        return Err("jp2: кодстрим без SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(from_siz[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, xo, yo) = (be32(6), be32(10), be32(14), be32(18));
    if xsiz <= xo || ysiz <= yo {
        return Err("jp2: пустой холст в SIZ".to_string());
    }
    Ok((xsiz - xo, ysiz - yo))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Бокс с телом — как их пишет контейнер.
    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn jp2_head(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr_body = height.to_be_bytes().to_vec();
        ihdr_body.extend_from_slice(&width.to_be_bytes());
        ihdr_body.extend_from_slice(&[0, 3, 7, 7, 0, 0]); // каналы, бит, прочее

        let mut head = JP2_MAGIC.to_vec();
        head.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]); // хвост сигнатурного бокса
        head.extend_from_slice(&boxed(b"ftyp", b"jp2 \x00\x00\x00\x00jp2 "));
        head.extend_from_slice(&boxed(b"jp2h", &boxed(b"ihdr", &ihdr_body)));
        head
    }

    #[test]
    fn container_header_yields_dims() {
        assert_eq!(header_dims(&jp2_head(10980, 5490)), Ok((10980, 5490)));
    }

    #[test]
    fn raw_codestream_yields_dims() {
        // SOC + SIZ с холстом 700×40 без смещения.
        let mut head = CODESTREAM_MAGIC.to_vec();
        head.extend_from_slice(&41u16.to_be_bytes()); // Lsiz
        head.extend_from_slice(&0u16.to_be_bytes()); // Rsiz
        for value in [700u32, 40, 0, 0] {
            head.extend_from_slice(&value.to_be_bytes());
        }
        assert_eq!(header_dims(&head), Ok((700, 40)));
    }

    #[test]
    fn garbage_is_refused() {
        assert!(header_dims(b"PNG not really").is_err());
        assert!(header_dims(JP2_MAGIC).is_err()); // сигнатура есть, заголовка нет
    }

    #[test]
    fn ladder_level_locates_actual_decode_step() {
        // Нечётные стороны: ceil-деление, как у pyramid::level_size.
        assert_eq!(ladder_level(1301, 523, 1301, 523, 3), Some(0));
        assert_eq!(ladder_level(1301, 523, 651, 262, 3), Some(1));
        // Грубее запрошенного декод не бывает — глубже up_to не ищется.
        assert_eq!(ladder_level(1301, 523, 326, 131, 1), None);
        // floor-лестница (650 = 1301/2 с округлением вниз) — не наша.
        assert_eq!(ladder_level(1301, 523, 650, 262, 3), None);
    }

    #[test]
    fn budget_refuses_native_tci_but_takes_overview() {
        // TCI Sentinel-2: 10980² в родном разрешении — за потолком,
        // уровень 2 — в бюджете.
        assert!(estimate(120 << 20, 10980, 10980) > DECODE_BUDGET);
        assert!(estimate(120 << 20, 2745, 2745) <= DECODE_BUDGET);
    }

    #[test]
    fn margins_key_edge_connected_zeros_only() {
        // 3×3: нулевой угол — поле, нулевой центр — настоящий чёрный.
        let mut rgba = Vec::new();
        for v in [0u8, 5, 5, 5, 0, 5, 5, 5, 5] {
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
        key_margins(&mut rgba, 3, 3);
        let alphas: Vec<u8> = rgba.chunks(4).map(|px| px[3]).collect();
        assert_eq!(alphas, vec![0, 255, 255, 255, 255, 255, 255, 255, 255]);
        // Цвет ключёванного не трогается: усреднение взвешено по альфе, и
        // кромёжный пиксель не обязан быть чёрным заранее.
        assert_eq!(&rgba[..3], &[0, 0, 0]);
    }
}
