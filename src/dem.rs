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
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    
    if extension == "pgm" {
        load_pgm(path, lat_start, lon_start, stride)
    } else {
        load_tiff(path, lat_start, lon_start, stride)
    }
}

fn load_tiff(path: &Path, lat_start: f32, lon_start: f32, stride: usize) -> anyhow::Result<DemData> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(file)?;
    let (w, h) = decoder.dimensions()?;
    
    let data: Vec<f32> = match decoder.read_image()? {
        DecodingResult::F32(v) => v,
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
        _ => return Err(anyhow::anyhow!("Unsupported TIFF format")),
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

fn load_pgm(path: &Path, lat_start: f32, lon_start: f32, stride: usize) -> anyhow::Result<DemData> {
    use std::io::{BufRead, BufReader, Read};
    
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim() != "P5" {
        return Err(anyhow::anyhow!("Invalid PGM format, expected P5"));
    }
    
    // Skip comments
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        if !line.starts_with('#') {
            break;
        }
    }
    
    // Read dimensions
    let dims: Vec<usize> = line.split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    let w = dims[0];
    let h = dims[1];
    
    line.clear();
    reader.read_line(&mut line)?;
    let max_val: u16 = line.trim().parse()?;
    
    let mut raw_data = Vec::new();
    reader.read_to_end(&mut raw_data)?;
    
    let mut heights = Vec::new();
    let new_w = w / stride;
    let new_h = h / stride;
    
    for y in 0..new_h {
        for x in 0..new_w {
            let idx = ((y * stride) * w + (x * stride)) * 2;
            if idx + 1 < raw_data.len() {
                // P5 is typically big-endian if max_val > 255
                let val = if max_val > 255 {
                    u16::from_be_bytes([raw_data[idx], raw_data[idx+1]])
                } else {
                    raw_data[idx] as u16
                };
                
                // Geoid heights in EGM2008 PGM are often offset
                // For now just cast to f32. In a real scenario we'd need the offset/scale from the comment.
                heights.push(val as f32);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_load_dem_file_not_found() {
        let path = PathBuf::from("non_existent.tif");
        let result = load_dem(&path, 0.0, 0.0, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_dem_valid() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("data/test_tile.tif");
        
        // This test might fail if the file is not a valid TIFF with F32 data
        // which is good for the "Red" phase if it's not implemented correctly.
        let result = load_dem(&path, 47.0, 39.0, 1);
        assert!(result.is_ok(), "Failed to load DEM: {:?}", result.err());
        
        let dem = result.unwrap();
        assert!(dem.width > 0);
        assert!(dem.height > 0);
        assert_eq!(dem.heights.len(), dem.width * dem.height);
    }

    #[test]
    fn test_load_geoid_pgm() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("data/geoids/egm2008-5.pgm");
        
        let result = load_dem(&path, 0.0, 0.0, 1);
        assert!(result.is_ok(), "Failed to load geoid PGM: {:?}", result.err());
        
        let dem = result.unwrap();
        assert_eq!(dem.width, 4320);
        assert_eq!(dem.height, 2161);
    }
}
