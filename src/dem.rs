use tiff::decoder::{Decoder, DecodingResult};
use std::fs::File;
use std::path::Path;

pub struct DemData {
    pub heights: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub lat_start: f32,
    pub lon_start: f32,
}

pub fn load_dem(path: &Path, lat_start: f32, lon_start: f32, stride: usize) -> anyhow::Result<DemData> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(file)?;
    let (w, h) = decoder.dimensions()?;
    
    let data = match decoder.read_image()? {
        DecodingResult::F32(v) => v,
        _ => return Err(anyhow::anyhow!("Expected F32 TIFF data")),
    };

    let new_w = w as usize / stride;
    let new_h = h as usize / stride;
    let mut heights = Vec::with_capacity(new_w * new_h);

    for y in 0..new_h {
        for x in 0..new_w {
            let idx = (y * stride) * w as usize + (x * stride);
            let val = data[idx];
            // Обработка NoData (у Copernicus DEM это обычно очень маленькие числа)
            if val < -1000.0 {
                heights.push(0.0);
            } else {
                heights.push(val);
            }
        }
    }

    Ok(DemData {
        heights,
        width: new_w,
        height: new_h,
        lat_start,
        lon_start,
    })
}
