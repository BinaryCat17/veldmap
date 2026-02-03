use anyhow::Result;
use std::io::Cursor;
use log::info;

pub fn decode_tiff(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    info!("WASM: Decoding TIFF ({} bytes)", data.len());
    
    // Пытаемся использовать стандартный декодер image crate сначала (для обычных TIFF)
    if let Ok(img) = image::load_from_memory_with_format(data, image::ImageFormat::Tiff) {
        let mut img = img;
        let (w, h) = (img.width(), img.height());
        info!("WASM: TIFF decoded via image crate ({}x{})", w, h);
        
        if w > 2048 || h > 2048 {
            info!("WASM: Resizing via image crate...");
            img = img.thumbnail(2048, 2048);
        }
        
        let rgba = img.to_rgba8();
        return Ok((rgba.width(), rgba.height(), rgba.into_raw()));
    }

    // Если не вышло (например, 32-bit float TIFF), используем специализированный tiff crate
    use tiff::decoder::{Decoder, DecodingResult};
    
    let cursor = Cursor::new(data);
    let mut decoder = Decoder::new(cursor)?;
    let (w, h) = decoder.dimensions()?;
    info!("WASM: Scientific TIFF dimensions: {}x{}", w, h);

    // Читаем изображение
    let pixels_f32: Vec<f32> = match decoder.read_image()? {
        DecodingResult::F32(v) => v,
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U8(v) => v.into_iter().map(|x| x as f32).collect(),
        _ => return Err(anyhow::anyhow!("Unsupported TIFF data format")),
    };

    // Определяем коэффициент уменьшения
    let factor = (w.max(h) as f32 / 2048.0).ceil() as u32;
    let new_w = w / factor;
    let new_h = h / factor;
    
    let total_pixels = (w as usize) * (h as usize);
    let samples_per_pixel = if total_pixels > 0 { pixels_f32.len() / total_pixels } else { 0 };
    if samples_per_pixel == 0 { return Err(anyhow::anyhow!("Invalid TIFF dimensions")); }

    info!("WASM: Processing and downsampling (factor {}) to {}x{}", factor, new_w, new_h);

    // Pass 1: Находим min/max для контраста (используем только первый канал)
    let mut min_val = f32::MAX;
    let mut max_val = f32::MIN;
    for i in (0..pixels_f32.len()).step_by(samples_per_pixel) {
        let val = pixels_f32[i];
        if val > -1000.0 && val < 10000.0 {
            if val < min_val { min_val = val; }
            if val > max_val { max_val = val; }
        }
    }
    if min_val >= max_val { max_val = min_val + 1.0; }
    let range = max_val - min_val;

    // Pass 2: Создаем уменьшенное RGBA изображение
    let mut rgba_data = Vec::with_capacity((new_w * new_h * 4) as usize);
    for y in 0..new_h {
        for x in 0..new_w {
            let old_idx = ((y * factor * w) + (x * factor)) as usize * samples_per_pixel;
            let val = pixels_f32[old_idx];
            
            if val <= -1000.0 {
                rgba_data.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let norm = ((val - min_val) / range).clamp(0.0, 1.0);
                let v = (norm * 255.0) as u8;
                rgba_data.extend_from_slice(&[v, v, v, 255]);
            }
        }
    }

    Ok((new_w, new_h, rgba_data))
}
