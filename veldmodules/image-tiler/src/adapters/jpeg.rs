//! JPEG: the low level is nearly free — the decoder can hand back a frame
//! reduced right in DCT (down to 1/8 per side). The frame is decoded at the
//! scale of the requested level, brought to the exact level grid, and sent into
//! the cascade from that level down: finer levels come from the same pass, while
//! coarser ones need a separate request — JPEGs are small by nature here, and
//! re-decoding is cheaper than always expanding the full frame.

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
    // The output size is asked of `scale`, not of `info` after the decode: the ceiling
    // must fire BEFORE the frame is allocated — otherwise it rejects what is already
    // paid for, and never fires on the very frame it exists for.
    // The decoder rounds the scale to eighths, so the output can be larger than the
    // requested level — the ceiling measures what will come out, not what was asked.
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
        // 16 bits per channel: the high byte, as in the other formats.
        jpeg_decoder::PixelFormat::L16 => {
            let bytes: Vec<u8> = pixels.chunks_exact(2).map(|p| p[1]).collect();
            to_rgba(&bytes, Pixel::named(1), (dw as usize) * (dh as usize))
        }
        other => return Err(format!("jpeg: pixel format {:?} is not supported", other)),
    };
    // The raw frame is no longer needed, and it lives at up to three bytes per pixel.
    // Next come the resample to the level size and the cascade, each asking for its
    // own copy: keeping the raw one beside them would mean paying for the frame thrice.
    drop(pixels);

    // DCT scales are discrete, the level grid is not: the frame is brought to the
    // exact level size, otherwise the tiles would disagree with the pyramid arithmetic.
    let rgba = if (dw, dh) != (lw, lh) { resample(&rgba, dw, dh, lw, lh) } else { rgba };

    let mut cascade = Cascade::new(level, lw, lh);
    cascade.push_rows(&rgba, lh, emit)?;
    cascade.finish(emit)
}

fn clamp_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}

/// The frame size the decoder will hand back for a level: its scales are
/// discrete — 1/8, 1/4, 1/2, and whole — and the first one is taken whose at
/// least one side is not smaller than the level side (that is how `Decoder::scale`
/// picks; the output side is `ceil(side · scale)`). It is computed in advance
/// into the level table so the frame ceiling lands in the "fits" column instead
/// of a refusal on every request; the value must match what the decoder produces.
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
