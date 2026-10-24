pub struct Frame {
    pub zone: u32,
    pub south: bool,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

const TILE_SIDE_M: f64 = 109_800.0;
const SQUARE_M: f64 = 100_000.0;
const ROW_CYCLE_M: f64 = 2_000_000.0;
const BAND_DEG: f64 = 8.0;
const LAST_BAND_DEG: f64 = 12.0;

const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";
const ROWS: &[u8] = b"ABCDEFGHJKLMNPQRSTUV";

fn band_middle_deg(band: usize) -> f64 {
    let south_edge = -80.0 + band as f64 * BAND_DEG;
    let height = match band == BANDS.len() - 1 {
        true => LAST_BAND_DEG,
        false => BAND_DEG,
    };
    south_edge + height / 2.0
}

pub fn tile_of(product_name: &str) -> Option<&str> {
    product_name.split('_').find_map(|part| {
        let tile = part.strip_prefix('T')?;
        let bytes = tile.as_bytes();
        let ok = bytes.len() == 5
            && bytes[..2].iter().all(u8::is_ascii_digit)
            && bytes[2..].iter().all(u8::is_ascii_uppercase);
        ok.then_some(tile)
    })
}

pub fn frame(tile: &str) -> Result<Frame, String> {
    let bytes = tile.as_bytes();
    if bytes.len() != 5 {
        return Err(format!("mgrs: tile code '{}' is not five characters", tile));
    }
    let zone: u32 = tile[..2].parse().map_err(|_| format!("mgrs: zone in '{}' is not a number", tile))?;
    if !(1..=60).contains(&zone) {
        return Err(format!("mgrs: there is no zone {}", zone));
    }
    let band = BANDS
        .iter()
        .position(|&b| b == bytes[2])
        .ok_or_else(|| format!("mgrs: there is no latitude band '{}'", bytes[2] as char))?;

    let column_set: &[u8] = match (zone - 1) % 3 {
        0 => b"ABCDEFGH",
        1 => b"JKLMNPQR",
        _ => b"STUVWXYZ",
    };
    let column = column_set
        .iter()
        .position(|&b| b == bytes[3])
        .ok_or_else(|| format!("mgrs: there is no column '{}' in zone {}", bytes[3] as char, zone))?;
    let x0 = (column as f64 + 1.0) * SQUARE_M;

    let row = ROWS
        .iter()
        .position(|&b| b == bytes[4])
        .ok_or_else(|| format!("mgrs: there is no row '{}'", bytes[4] as char))?;
    let shift = if zone % 2 == 0 { 5 } else { 0 };
    let base = ((row + ROWS.len() - shift) % ROWS.len()) as f64 * SQUARE_M;

    let band_mid_deg = band_middle_deg(band);
    let south = band_mid_deg < 0.0;
    let approx = band_mid_deg.abs() * 110_946.0;
    let target = if south { 10_000_000.0 - approx } else { approx };
    let mut northing = base;
    while northing + ROW_CYCLE_M / 2.0 < target {
        northing += ROW_CYCLE_M;
    }

    let y1 = northing + SQUARE_M;
    Ok(Frame { zone, south, x0, y0: y1 - TILE_SIDE_M, x1: x0 + TILE_SIDE_M, y1 })
}
