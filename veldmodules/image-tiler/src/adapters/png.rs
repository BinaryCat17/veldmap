//! PNG: один zlib-поток, произвольного доступа нет — только полный проход.
//! Строки уезжают в каскад по мере декодирования, поэтому тайлы уровней
//! появляются сверху вниз, а память не зависит от высоты файла.

use std::io::{BufRead, Seek};

use super::super::cascade::{Cascade, Emit};
use super::{to_rgba, Info, Kind, FULL_DECODE_BUDGET};

pub fn describe<R: BufRead + Seek>(reader: R) -> Result<Info, String> {
    let decoder = png::Decoder::new(reader);
    let reader = decoder.read_info().map_err(|e| format!("png: {}", e))?;
    let info = reader.info();
    Ok(Info { width: info.width, height: info.height, kind: Kind::Png })
}

pub fn produce_pass<R: BufRead + Seek>(reader: R, info: &Info, emit: Emit) -> Result<(), String> {
    let mut decoder = png::Decoder::new(reader);
    // Палитра разворачивается, 1/2/4 бита дополняются, 16 бит ужимаются до
    // 8 — ниже остаются только четыре комбинации каналов.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("png: {}", e))?;

    let (w, h, interlaced) = {
        let i = reader.info();
        (i.width, i.height, i.interlaced)
    };
    if (w, h) != (info.width, info.height) {
        return Err("png: размеры разошлись с заголовком".to_string());
    }

    let mut cascade = Cascade::new(0, w, h);

    // У чересстрочного PNG строки идут в порядке Adam7, а не сверху вниз:
    // потокового пути нет, кадр декодируется целиком — и потому с бюджетом.
    if interlaced {
        if u64::from(w) * u64::from(h) * 4 > FULL_DECODE_BUDGET {
            return Err(format!(
                "png (Adam7) {}×{}: кадр целиком не влезает в бюджет ({} МБ)",
                w, h, FULL_DECODE_BUDGET / (1024 * 1024)
            ));
        }
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| "png: кадр не помещается в адресное пространство".to_string())?;
        let mut buf = vec![0u8; size];
        let out = reader.next_frame(&mut buf).map_err(|e| format!("png: {}", e))?;
        let channels = usize::from(out.color_type.samples());
        let rgba = to_rgba(&buf, channels, (w as usize) * (h as usize));
        cascade.push_rows(&rgba, h, emit)?;
        return cascade.finish(emit);
    }

    let channels = usize::from(reader.output_color_type().0.samples());
    while let Some(row) = reader.next_row().map_err(|e| format!("png: {}", e))? {
        let rgba = to_rgba(row.data(), channels, w as usize);
        cascade.push_rows(&rgba, 1, emit)?;
    }
    cascade.finish(emit)
}
