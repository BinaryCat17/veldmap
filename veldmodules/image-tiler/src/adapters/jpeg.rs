//! JPEG: почти дешёвый низкий уровень — декодер умеет отдавать кадр
//! уменьшенным прямо в DCT (до 1/8 по стороне). Кадр декодируется в масштаб
//! запрошенного уровня, приводится к точной сетке уровня и уезжает в каскад
//! с этого уровня вниз: более мелкие уровни достаются тем же проходом, а за
//! более крупными придёт отдельный запрос — JPEG здесь малы по природе, и
//! повторный декод дешевле, чем всегда разворачивать полный кадр.

use std::io::Read;

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid;
use super::super::resample::resample;
use super::radiometry::Pixel;
use super::{frame_fits, to_rgba, Info, Kind, FULL_DECODE_BUDGET};

pub fn describe<R: Read>(reader: R) -> Result<Info, String> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    let info = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;
    Ok(Info::plain(u32::from(info.width), u32::from(info.height), Kind::Jpeg))
}

pub fn produce<R: Read>(reader: R, info: &Info, level: u32, emit: Emit) -> Result<(), String> {
    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);

    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    // Размер выхода спрашивается у `scale`, а не у `info` после декода: потолок
    // обязан сработать ДО того, как кадр выделен, — иначе он отвергает уже
    // оплаченное, а на кадре, ради которого заведён, не срабатывает вовсе.
    // Масштаб декодер округляет до восьмых долей, поэтому выход бывает крупнее
    // запрошенного уровня — потолок меряет то, что выйдет, а не то, что просили.
    let (dw, dh) = decoder
        .scale(clamp_u16(lw), clamp_u16(lh))
        .map(|(w, h)| (u32::from(w), u32::from(h)))
        .map_err(|e| format!("jpeg: {}", e))?;
    if !frame_fits(dw, dh) {
        return Err(format!(
            "jpeg {}×{}: кадр целиком не влезает в бюджет ({} МБ)",
            dw, dh, FULL_DECODE_BUDGET / (1024 * 1024)
        ));
    }
    let pixels = decoder.decode().map_err(|e| format!("jpeg: {}", e))?;
    let dinfo = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;

    let rgba = match dinfo.pixel_format {
        jpeg_decoder::PixelFormat::L8 => to_rgba(&pixels, Pixel::named(1), (dw as usize) * (dh as usize)),
        jpeg_decoder::PixelFormat::RGB24 => to_rgba(&pixels, Pixel::named(3), (dw as usize) * (dh as usize)),
        // 16 бит на канал: старший байт, как у остальных форматов.
        jpeg_decoder::PixelFormat::L16 => {
            let bytes: Vec<u8> = pixels.chunks_exact(2).map(|p| p[1]).collect();
            to_rgba(&bytes, Pixel::named(1), (dw as usize) * (dh as usize))
        }
        other => return Err(format!("jpeg: формат пикселей {:?} не поддерживается", other)),
    };
    // Сырой кадр больше не нужен, а живёт он до трёх байт на пиксель. Дальше
    // идут приведение к размерам уровня и каскад, и каждое из них просит своё:
    // держать рядом с ними ещё и сырьё значило бы платить за кадр трижды.
    drop(pixels);

    // Масштабы DCT дискретны, а сетка уровня — нет: кадр приводится к точным
    // размерам уровня, иначе тайлы разошлись бы с арифметикой пирамиды.
    let rgba = if (dw, dh) != (lw, lh) { resample(&rgba, dw, dh, lw, lh) } else { rgba };

    let mut cascade = Cascade::new(level, lw, lh);
    cascade.push_rows(&rgba, lh, emit)?;
    cascade.finish(emit)
}

fn clamp_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}

/// Размер кадра, который декодер отдаст под уровень: масштабы у него
/// дискретные — 1/8, 1/4, 1/2 и целый, — и берётся первый, у которого хотя бы
/// одна сторона не меньше стороны уровня (так выбирает `Decoder::scale`;
/// сторона выхода — `ceil(сторона · масштаб)`). Считается заранее таблицей
/// уровней, чтобы потолок кадра лёг в столбец «влезает», а не в отказ на
/// каждый запрос; сходство с декодером держит тест.
pub(super) fn decoded_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    let lw = pyramid::level_size(width, level);
    let lh = pyramid::level_size(height, level);
    let scaled = |side: u32, eighths: u32| (u64::from(side) * u64::from(eighths)).div_ceil(8) as u32;
    let eighths = [1u32, 2, 4]
        .into_iter()
        .find(|&e| scaled(width, e) >= lw || scaled(height, e) >= lh)
        .unwrap_or(8);
    (scaled(width, eighths), scaled(height, eighths))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Заголовок JPEG без единого байта сканирования: размеры объявлены, а
    /// декодировать нечего.
    fn header(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.push(3);
        for id in 1..=3u8 {
            bytes.extend_from_slice(&[id, 0x11, 0x00]);
        }
        bytes
    }

    /// Потолок кадра срабатывает ДО декода, а не после него.
    ///
    /// Проверяется здесь именно порядок, и заголовок без данных сканирования —
    /// то, чем его видно: сработай потолок после, декодер упёрся бы в конец
    /// файла и сказал бы про него, а не про бюджет. Порядок этот и есть весь
    /// смысл проверки: кадр, ради которого она написана, не влезает в память
    /// инстанса, то есть выделить его значит упасть, ничего не сказав.
    #[test]
    fn потолок_кадра_срабатывает_до_декода() {
        let huge = header(20_000, 20_000);
        let info = Info::plain(20_000, 20_000, Kind::Jpeg);
        let mut emit = |_: u32, _: u32, _: u32, _: u32, _: u32, _: &[u8]| Ok(());

        let why = produce(huge.as_slice(), &info, 0, &mut emit)
            .expect_err("кадр 20000×20000 обязан быть отвергнут");
        assert!(why.contains("не влезает в бюджет"), "отказ не про бюджет: {why}");
    }

    /// Размер кадра под уровень считается так же, как его выберет декодер:
    /// иначе столбец «влезает» таблицы уровней обещал бы кадр, которого
    /// декодер не даст. Проверяется на заголовке без данных сканирования —
    /// `scale` читает только его.
    #[test]
    fn decoded_size_matches_the_decoder() {
        for (w, h) in [(5472u16, 3648u16), (4000, 3000), (1301, 523), (64, 64)] {
            let head = header(w, h);
            for level in 0..pyramid::level_count(u32::from(w), u32::from(h)) + 1 {
                let mut decoder = jpeg_decoder::Decoder::new(head.as_slice());
                decoder.read_info().unwrap();
                let (lw, lh) = (
                    pyramid::level_size(u32::from(w), level),
                    pyramid::level_size(u32::from(h), level),
                );
                let got = decoder.scale(clamp_u16(lw), clamp_u16(lh)).unwrap();
                assert_eq!(
                    decoded_size(u32::from(w), u32::from(h), level),
                    (u32::from(got.0), u32::from(got.1)),
                    "{w}×{h}, уровень {level}"
                );
            }
        }
    }

    /// А кадр по размеру потолок не трогает — и тогда до декода дело доходит,
    /// что по отказу и видно. Без этой половины первый тест доказывал бы лишь,
    /// что `produce` всегда отказывает.
    #[test]
    fn кадр_по_размеру_до_потолка_не_доходит() {
        let small = header(64, 64);
        let info = Info::plain(64, 64, Kind::Jpeg);
        let mut emit = |_: u32, _: u32, _: u32, _: u32, _: u32, _: &[u8]| Ok(());

        let why = produce(small.as_slice(), &info, 0, &mut emit)
            .expect_err("данных сканирования в заголовке нет, декод обязан сорваться");
        assert!(!why.contains("бюджет"), "мелкий кадр отвергнут потолком: {why}");
    }
}
