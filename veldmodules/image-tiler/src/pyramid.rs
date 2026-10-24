pub const TILE: u32 = 512;

pub fn level_size(side: u32, level: u32) -> u32 {
    let mut size = side;
    for _ in 0..level {
        size = size.div_ceil(2);
    }
    size
}

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

pub fn grid(side_px: u32) -> u32 {
    side_px.div_ceil(TILE)
}

pub fn tile_extent(index: u32, side_px: u32) -> u32 {
    let start = index.saturating_mul(TILE);
    side_px.min(start.saturating_add(TILE)).saturating_sub(start)
}

pub fn level_px(level: u32) -> f64 {
    (1u64 << level) as f64
}

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

pub fn cell_uv(cell: [f64; 4], carrier: (u32, u32, u32), tex_w: u32, tex_h: u32) -> [f32; 4] {
    let (level, x, y) = carrier;
    let px = level_px(level);
    let origin_x = f64::from(x) * f64::from(TILE);
    let origin_y = f64::from(y) * f64::from(TILE);
    let u = |ix: f64| ((ix / px - origin_x) / f64::from(tex_w)).clamp(0.0, 1.0) as f32;
    let v = |iy: f64| ((iy / px - origin_y) / f64::from(tex_h)).clamp(0.0, 1.0) as f32;
    [u(cell[0]), v(cell[1]), u(cell[2]), v(cell[3])]
}
