use anyhow::Result;
use image::{ImageBuffer, Rgba, ImageFormat};
use std::fs::File;
use std::io::Cursor;
use log::{info, error};

pub fn generate_preview(file_path: &std::path::Path) -> Result<Vec<u8>> {
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    info!("Generating preview for: {:?} (ext: {})", file_path, ext);
    
    if ext == "tif" || ext == "tiff" {
        use tiff::decoder::{Decoder, DecodingResult};
        let file = File::open(file_path)?;
        let mut decoder = Decoder::new(file)?;
        let (w, h) = decoder.dimensions()?;

        let data = match decoder.read_image() {
            Ok(DecodingResult::F32(v)) => v,
            Ok(DecodingResult::I16(v)) => v.into_iter().map(|x| x as f32).collect(),
            Ok(DecodingResult::U16(v)) => v.into_iter().map(|x| x as f32).collect(),
            Ok(DecodingResult::U8(v)) => v.into_iter().map(|x| x as f32).collect(),
            Err(e) => {
                error!("TIFF decode error: {:?}", e);
                return Err(anyhow::anyhow!("TIFF decode error: {:?}", e));
            }
            _ => return Err(anyhow::anyhow!("Unsupported TIFF data format")),
        };

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

        let mut png_data = Vec::new();
        img_buf.write_to(&mut Cursor::new(&mut png_data), ImageFormat::Png)?;
        return Ok(png_data);
    }

    let img = image::open(file_path)?;
    let mut png_data = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_data), ImageFormat::Png)?;
    Ok(png_data)
}
