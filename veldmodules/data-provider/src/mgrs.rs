//! MGRS grid: Sentinel-2 tile code → granule frame in meters of its UTM zone.
//!
//! An S2 granule is a 109,800 m square whose upper-left corner stands at the
//! corner of an MGRS 100 km square (the southwest corner of a 100×100 km
//! square plus 100 km north); neighboring granules overlap because the corners
//! step by 100 km while the square is wider. Neither the catalogue nor the
//! listing reports these numbers — they are baked into the tile name itself
//! (`T40WFC`), and it is parsed by whoever knows the product layout, that is,
//! this module. There is no projection math here: the frame is returned in the
//! zone's numbers, and the consumer (the globe) converts them to degrees.
//!
//! The letter arithmetic follows the grid definition: columns go in eights
//! A–H / J–R / S–Z on a cycle of three zones, rows in twenties A–V (no I or O)
//! with a half shift in even zones, and repeat every 2,000,000 m. The repeat
//! is resolved by the latitude band from the same tile code.

/// Raster frame in its UTM zone. `y1` is the northern edge: raster rows run
/// north to south, as granule files store them.
pub struct Frame {
    pub zone: u32,
    pub south: bool,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

/// Sentinel-2 granule side, meters: 10980 pixels of 10 m.
const TILE_SIDE_M: f64 = 109_800.0;
/// Side of an MGRS 100 km square.
const SQUARE_M: f64 = 100_000.0;
/// Repetition period of a row letter.
const ROW_CYCLE_M: f64 = 2_000_000.0;
/// Latitude band height, degrees.
const BAND_DEG: f64 = 8.0;
/// Height of band X — the only one wider than the rest: it covers the Arctic
/// up to 84°.
const LAST_BAND_DEG: f64 = 12.0;

/// Latitude band letters, south to north in 8° steps; I and O are skipped.
const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";
/// Row letters of the 100 km squares, a cycle of twenty; I and O are skipped.
const ROWS: &[u8] = b"ABCDEFGHJKLMNPQRSTUV";

/// Middle of a latitude band by its number, degrees.
///
/// The bands run in 8° steps from −80°, and only the last one, X, is twelve
/// degrees wide: it covers the Arctic up to 84° without introducing another
/// letter. Compute its middle by the general rule — and the row-selection
/// window slides two degrees south: the northern edge of the band keeps 112 km
/// of margin instead of 354. That is still enough for now, but the margin,
/// shrunk threefold against the other bands, already rests on nothing.
fn band_middle_deg(band: usize) -> f64 {
    let south_edge = -80.0 + band as f64 * BAND_DEG;
    let height = match band == BANDS.len() - 1 {
        true => LAST_BAND_DEG,
        false => BAND_DEG,
    };
    south_edge + height / 2.0
}

/// The tile code from a product name: the `_TxxABC_` segment with a zone and
/// three letters.
///
/// The name comes from storage, so the length is checked before any slice and
/// in the same condition with them: not only the granule code starts with `T`,
/// but also, say, the Landsat tier (`..._T1`), and slicing past the length is
/// a panic, not a refusal. Slicing is by bytes: the five-character code is
/// Latin, and a byte slice does not ask about character boundaries.
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

/// Granule frame by tile code (`40WFC`).
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

    // Column: three sets of eight letters, the zone picks the set.
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

    // Row: the letter gives northing to within a 2000 km period; even zones
    // are shifted by five letters. The period is resolved by the middle of the
    // latitude band: a crude meters-per-degree estimate is enough — from the
    // middle to the band edge is 444 km (666 for band X), that is, three times
    // the traveled distance remains to half the period, and the error of the
    // linear approximation (up to 12 km at the 84th parallel) fits into that
    // margin with room to spare.
    let row = ROWS
        .iter()
        .position(|&b| b == bytes[4])
        .ok_or_else(|| format!("mgrs: there is no row '{}'", bytes[4] as char))?;
    let shift = if zone % 2 == 0 { 5 } else { 0 };
    let base = ((row + ROWS.len() - shift) % ROWS.len()) as f64 * SQUARE_M;

    let band_mid_deg = band_middle_deg(band);
    let south = band_mid_deg < 0.0;
    let approx = band_mid_deg.abs() * 110_946.0;
    // Northing candidates for the southern edge of the 100 km square: from the
    // equator for northern bands, downward from 10,000,000 for southern ones
    // (the false northing of southern zones).
    let target = if south { 10_000_000.0 - approx } else { approx };
    let mut northing = base;
    while northing + ROW_CYCLE_M / 2.0 < target {
        northing += ROW_CYCLE_M;
    }

    let y1 = northing + SQUARE_M;
    Ok(Frame { zone, south, x0, y0: y1 - TILE_SIDE_M, x1: x0 + TILE_SIDE_M, y1 })
}
