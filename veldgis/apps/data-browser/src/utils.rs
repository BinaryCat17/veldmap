use anyhow::Result;
use image::{ImageBuffer, Rgba};
use std::io::Cursor;

pub fn decode_tiff(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    use tiff::decoder::{Decoder, DecodingResult};
    
    let cursor = Cursor::new(data);
    let mut decoder = Decoder::new(cursor)?;
    let (w, h) = decoder.dimensions()?;

    let pixels_f32 = match decoder.read_image() {
        Ok(DecodingResult::F32(v)) => v,
        Ok(DecodingResult::I16(v)) => v.into_iter().map(|x| x as f32).collect(),
        Ok(DecodingResult::U16(v)) => v.into_iter().map(|x| x as f32).collect(),
        Ok(DecodingResult::U8(v)) => v.into_iter().map(|x| x as f32).collect(),
        Err(e) => return Err(anyhow::anyhow!("TIFF decode error: {:?}", e)),
        _ => return Err(anyhow::anyhow!("Unsupported TIFF data format")),
    };

    // Автоматический контраст
    let mut min_val = f32::MAX;
    let mut max_val = f32::MIN;
    for &val in &pixels_f32 {
        if val > -1000.0 && val < 10000.0 {
            if val < min_val { min_val = val; }
            if val > max_val { max_val = val; }
        }
    }

    if min_val >= max_val { max_val = min_val + 1.0; }
    let range = max_val - min_val;
    let mut img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(w, h);

    for (i, val) in pixels_f32.iter().enumerate() {
        let x = (i as u32) % w;
        let y = (i as u32) / w;
        
        let pixel = if *val <= -1000.0 {
            Rgba([0, 0, 0, 0]) // NoData
        } else {
            let norm = ((*val - min_val) / range).clamp(0.0, 1.0);
            let v = (norm * 255.0) as u8;
            Rgba([v, v, v, 255])
        };
        img_buf.put_pixel(x, y, pixel);
    }

    Ok((w, h, img_buf.into_raw()))
}
