use anyhow::Result;
use image::{ImageBuffer, Rgba};
use std::fs::File;

#[derive(Clone, Debug)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn generate_preview_with_progress(
    file_path: &std::path::Path,
    progress_tx: Option<tokio::sync::mpsc::Sender<f32>>
) -> Result<RawImage> {
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let send_progress = |p: f32| {
        if let Some(tx) = &progress_tx {
            let _ = tx.try_send(p);
        }
    };

    if ext == "tif" || ext == "tiff" {
        use tiff::decoder::{Decoder, DecodingResult};
        
        send_progress(0.1); 
        let file = File::open(file_path)?;
        let mut decoder = Decoder::new(file)?;
        let (w, h) = decoder.dimensions()?;

        send_progress(0.2); 
        let data = match decoder.read_image() {
            Ok(DecodingResult::F32(v)) => v,
            Ok(DecodingResult::I16(v)) => v.into_iter().map(|x| x as f32).collect(),
            Ok(DecodingResult::U16(v)) => v.into_iter().map(|x| x as f32).collect(),
            Ok(DecodingResult::U8(v)) => v.into_iter().map(|x| x as f32).collect(),
            Err(e) => return Err(anyhow::anyhow!("TIFF decode error: {:?}", e)),
            _ => return Err(anyhow::anyhow!("Unsupported TIFF data format")),
        };

        send_progress(0.6); 
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        for &val in &data {
            if val > -1000.0 && val < 10000.0 {
                if val < min_val { min_val = val; }
                if val > max_val { max_val = val; }
            }
        }

        if min_val >= max_val { max_val = min_val + 1.0; }
        let range = max_val - min_val;
        let mut img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(w, h);

        send_progress(0.8); 
        for (i, val) in data.iter().enumerate() {
            let x = (i as u32) % w;
            let y = (i as u32) / w;
            
            let pixel = if *val <= -1000.0 {
                Rgba([0, 0, 0, 0])
            } else {
                let norm = ((*val - min_val) / range).clamp(0.0, 1.0);
                let v = (norm * 255.0) as u8;
                Rgba([v, v, v, 255])
            };
            img_buf.put_pixel(x, y, pixel);
        }

        send_progress(1.0);
        return Ok(RawImage {
            width: w,
            height: h,
            pixels: img_buf.into_raw(),
        });
    }

    send_progress(0.3);
    let img = image::open(file_path)?.to_rgba8();
    let (w, h) = img.dimensions();
    send_progress(1.0);
    Ok(RawImage {
        width: w,
        height: h,
        pixels: img.into_raw(),
    })
}