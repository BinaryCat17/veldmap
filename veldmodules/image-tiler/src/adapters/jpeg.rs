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
use super::{to_rgba, Info, Kind, FULL_DECODE_BUDGET};

pub fn describe<R: Read>(reader: R) -> Result<Info, String> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    let info = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;
    Ok(Info {
        width: u32::from(info.width),
        height: u32::from(info.height),
        kind: Kind::Jpeg,
    })
}

pub fn produce<R: Read>(reader: R, info: &Info, level: u32, emit: Emit) -> Result<(), String> {
    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);

    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    decoder
        .scale(clamp_u16(lw), clamp_u16(lh))
        .map_err(|e| format!("jpeg: {}", e))?;
    let pixels = decoder.decode().map_err(|e| format!("jpeg: {}", e))?;
    let dinfo = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;
    let (dw, dh) = (u32::from(dinfo.width), u32::from(dinfo.height));
    if u64::from(dw) * u64::from(dh) * 4 > FULL_DECODE_BUDGET {
        return Err(format!(
            "jpeg {}×{}: кадр целиком не влезает в бюджет ({} МБ)",
            dw, dh, FULL_DECODE_BUDGET / (1024 * 1024)
        ));
    }

    let rgba = match dinfo.pixel_format {
        jpeg_decoder::PixelFormat::L8 => to_rgba(&pixels, 1, (dw as usize) * (dh as usize)),
        jpeg_decoder::PixelFormat::RGB24 => to_rgba(&pixels, 3, (dw as usize) * (dh as usize)),
        // 16 бит на канал: старший байт, как у остальных форматов.
        jpeg_decoder::PixelFormat::L16 => {
            let bytes: Vec<u8> = pixels.chunks_exact(2).map(|p| p[1]).collect();
            to_rgba(&bytes, 1, (dw as usize) * (dh as usize))
        }
        other => return Err(format!("jpeg: формат пикселей {:?} не поддерживается", other)),
    };

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
