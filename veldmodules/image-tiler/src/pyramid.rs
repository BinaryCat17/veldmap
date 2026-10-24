//! Tile pyramid arithmetic — the single instance of the rules about how many
//! levels a raster has, what the grid of a level is, and how large the edge
//! tile is.
//!
//! The file is included twice: by the module (`crate::module::pyramid`) and by
//! the wrap crate (`veldmap_image_tiler_wrap::pyramid`, through `#[path]` in
//! wrap.rs — the same trick by which the code generator includes wrap.rs
//! itself). Producer and consumers compute from one and the same code:
//! computed separately the results would agree only as long as someone keeps
//! the two copies identical.
//!
//! Anything from `crate::` would break the wrap inclusion silently, and a
//! library here only fits if declared for both — the module and the wrap crate
//! (`rust.wrap_dependencies` in config.yaml). Right now the file needs
//! neither: it is about integers.

/// Tile side in pixels. A large tile means fewer events, files, and bind groups
/// for the same area. One for producer and consumers: this file is included
/// into the wrap crate as well (see the header), so both sides compute cell
/// arithmetic from here. In `Described.tile` it travels to whoever does not
/// share the file.
pub const TILE: u32 = 512;

/// Side size at a level: sequential halving with rounding up. The composition
/// of roundings matches a single division — ceil(ceil(s/2)/2) == ceil(s/4) —
/// so this is ceil(side / 2^level).
pub fn level_size(side: u32, level: u32) -> u32 {
    let mut size = side;
    for _ in 0..level {
        size = size.div_ceil(2);
    }
    size
}

/// How many levels a raster has: level 0 is the native resolution, the top is
/// the first level whose large side fits in one tile. An empty raster has no
/// levels — zero is where any computation over it ends here as well.
pub fn level_count(width: u32, height: u32) -> u32 {
    if width == 0 || height == 0 {
        return 0;
    }
    let mut side = width.max(height);
    let mut levels = 1;
    while side > TILE {
        side = side.div_ceil(2);
        levels += 1;
    }
    levels
}

/// Tiles along a side of length `side_px` (the level's side, not the source's).
pub fn grid(side_px: u32) -> u32 {
    side_px.div_ceil(TILE)
}

/// Actual side of the tile with index `index`: at the right and bottom edges
/// the tile is shorter. Zero — there is no tile with such an index on this
/// side.
pub fn tile_extent(index: u32, side_px: u32) -> u32 {
    let start = index.saturating_mul(TILE);
    side_px.min(start.saturating_add(TILE)).saturating_sub(start)
}

/// Side of a level pixel in source pixels.
pub fn level_px(level: u32) -> f64 {
    (1u64 << level) as f64
}

/// Rectangle of a level cell in source pixels. The right and bottom edges are
/// clipped by the source itself: the level's pixels cover it with the margin
/// of the rounding up.
pub fn cell_image_rect(x: u32, y: u32, level: u32, width: u32, height: u32) -> [f64; 4] {
    let px = level_px(level);
    let lw = level_size(width, level);
    let lh = level_size(height, level);
    let x0 = f64::from(x) * f64::from(TILE) * px;
    let y0 = f64::from(y) * f64::from(TILE) * px;
    let x1 = (x0 + f64::from(tile_extent(x, lw)) * px).min(f64::from(width));
    let y1 = (y0 + f64::from(tile_extent(y, lh)) * px).min(f64::from(height));
    [x0, y0, x1, y1]
}

/// The piece of the carrier tile `(level, column, row)` covering the cell
/// (`cell_image_rect`). For the exact tile this is the whole tile; for an
/// ancestor d levels up — its 1/4^d, and fractional edges are legal:
/// sampling along them is exactly the averaging the exact tile would give.
pub fn cell_uv(cell: [f64; 4], carrier: (u32, u32, u32), tex_w: u32, tex_h: u32) -> [f32; 4] {
    let (level, x, y) = carrier;
    let px = level_px(level);
    let origin_x = f64::from(x) * f64::from(TILE);
    let origin_y = f64::from(y) * f64::from(TILE);
    let u = |ix: f64| ((ix / px - origin_x) / f64::from(tex_w)).clamp(0.0, 1.0) as f32;
    let v = |iy: f64| ((iy / px - origin_y) / f64::from(tex_h)).clamp(0.0, 1.0) as f32;
    [u(cell[0]), v(cell[1]), u(cell[2]), v(cell[3])]
}
