//! Форматы без потокового пути (gif/bmp/webp): кадр декодируется целиком —
//! такие файлы малы по своей природе, и бюджет это закрепляет.

use std::io::{BufRead, Seek};

use image::ImageFormat;

use super::super::cascade::{Cascade, Emit};
use super::{frame_fits, Info, Kind, FULL_DECODE_BUDGET};

pub fn describe<R: BufRead + Seek>(reader: R, format: ImageFormat) -> Result<Info, String> {
    let decoder = image::ImageReader::with_format(reader, format)
        .into_decoder()
        .map_err(|e| format!("{:?}: {}", format, e))?;
    let (width, height) = image::ImageDecoder::dimensions(&decoder);
    Ok(Info::plain(width, height, Kind::Full(format)))
}

pub fn produce<R: BufRead + Seek>(
    reader: R,
    info: &Info,
    format: ImageFormat,
    emit: Emit,
) -> Result<(), String> {
    // Без BufReader: читатель уже буферизован своим окном (см. Metered поверх
    // ResourceReader) — обёртка завела бы второй буфер и копировала окно в
    // него целиком.
    let decoder = image::ImageReader::with_format(reader, format)
        .into_decoder()
        .map_err(|e| format!("{:?}: {}", format, e))?;
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    if !frame_fits(w, h) {
        return Err(format!(
            "{:?} {}×{}: кадр целиком не влезает в бюджет ({} МБ)",
            format, w, h, FULL_DECODE_BUDGET / (1024 * 1024)
        ));
    }
    if (w, h) != (info.width, info.height) {
        return Err(format!("{:?}: размеры разошлись с заголовком", format));
    }

    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("{:?}: {}", format, e))?
        .to_rgba8();

    let mut cascade = Cascade::new(0, w, h);
    cascade.push_rows(&rgba, h, emit)?;
    cascade.finish(emit)
}
