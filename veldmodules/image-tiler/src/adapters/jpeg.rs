use std::io::Read;

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid;
use super::super::resample::resample;
use super::radiometry::Pixel;
use super::{frame_fits, to_rgba, Info, Kind, FULL_DECODE_BUDGET};

pub fn describe<R: Read>(reader: R) -> Result<Info, String> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    let info = decoder.info().ok_or_else(|| "jpeg: no header".to_string())?;
    Ok(Info::plain(u32::from(info.width), u32::from(info.height), Kind::Jpeg))
}

pub fn produce<R: Read>(reader: R, info: &Info, level: u32, emit: Emit) -> Result<(), String> {
    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);

    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    let (dw, dh) = decoder
        .scale(clamp_u16(lw), clamp_u16(lh))
        .map(|(w, h)| (u32::from(w), u32::from(h)))
        .map_err(|e| format!("jpeg: {}", e))?;
    if !frame_fits(dw, dh) {
        return Err(format!(
            "jpeg {}×{}: the whole frame does not fit the budget ({} MB)",
            dw, dh, FULL_DECODE_BUDGET / (1024 * 1024)
        ));
    }
    let pixels = decoder.decode().map_err(|e| format!("jpeg: {}", e))?;
    let dinfo = decoder.info().ok_or_else(|| "jpeg: no header".to_string())?;

    let rgba = match dinfo.pixel_format {
        jpeg_decoder::PixelFormat::L8 => to_rgba(&pixels, Pixel::named(1), (dw as usize) * (dh as usize)),
        jpeg_decoder::PixelFormat::RGB24 => to_rgba(&pixels, Pixel::named(3), (dw as usize) * (dh as usize)),
        jpeg_decoder::PixelFormat::L16 => {
            let bytes: Vec<u8> = pixels.chunks_exact(2).map(|p| p[1]).collect();
            to_rgba(&bytes, Pixel::named(1), (dw as usize) * (dh as usize))
        }
        other => return Err(format!("jpeg: pixel format {:?} is not supported", other)),
    };
    drop(pixels);

    let rgba = if (dw, dh) != (lw, lh) { resample(&rgba, dw, dh, lw, lh) } else { rgba };

    let mut cascade = Cascade::new(level, lw, lh);
    cascade.push_rows(&rgba, lh, emit)?;
    cascade.finish(emit)
}

fn clamp_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}

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
