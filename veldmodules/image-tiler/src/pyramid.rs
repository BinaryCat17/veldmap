//! Арифметика пирамиды тайлов — единственный экземпляр правил о том, сколько
//! у растра уровней, какова сетка уровня и какого размера краевой тайл.
//!
//! Файл включён дважды: модулем (`crate::module::pyramid`) и wrap-крейтом
//! (`veldmap_image_tiler_wrap::pyramid`, через `#[path]` в wrap.rs — тем же
//! приёмом, каким кодогенератор включает сам wrap.rs). Производитель и
//! потребители считают по одному и тому же коду: посчитанное порознь
//! сходилось бы, только пока кто-то держит две копии одинаковыми.
//!
//! Зависимостей нет намеренно: файл обязан компилироваться в обоих крейтах.

/// Сторона тайла в пикселях. 512, а не 256: вдвое меньше событий, файлов и
/// bind group на ту же площадь. Константа производителя — потребители берут
/// её из `Described.tile`, а не отсюда; здесь она нужна самой арифметике.
pub const TILE: u32 = 512;

/// Размер стороны на уровне: последовательное деление пополам с округлением
/// вверх. Композиция округлений совпадает с одним делением —
/// ceil(ceil(s/2)/2) == ceil(s/4), — поэтому это ceil(side / 2^level).
pub fn level_size(side: u32, level: u32) -> u32 {
    let mut size = side;
    for _ in 0..level {
        size = size.div_ceil(2);
    }
    size
}

/// Сколько уровней у растра: уровень 0 — родное разрешение, вершина — первый
/// уровень, чья большая сторона помещается в один тайл. У пустого растра
/// уровней нет — нулём здесь кончается и любой расчёт по нему.
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

/// Тайлов вдоль стороны длиной `side_px` (сторона уровня, не источника).
pub fn grid(side_px: u32) -> u32 {
    side_px.div_ceil(TILE)
}

/// Фактическая сторона тайла с индексом `index`: у правого и нижнего краёв
/// тайл короче. Ноль — тайла с таким индексом на этой стороне нет.
pub fn tile_extent(index: u32, side_px: u32) -> u32 {
    let start = index.saturating_mul(TILE);
    side_px.min(start.saturating_add(TILE)).saturating_sub(start)
}

/// Сторона пикселя уровня в пикселях источника.
pub fn level_px(level: u32) -> f64 {
    (1u64 << level) as f64
}

/// Прямоугольник ячейки уровня в пикселях источника. Правый и нижний края
/// обрезаны самим источником: пиксели уровня накрывают его с запасом
/// округления вверх.
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

/// Кусок тайла-носителя `(уровень, колонка, ряд)`, накрывающий ячейку
/// (`cell_image_rect`). Для точного тайла это весь тайл; для предка на d
/// уровней выше — его 1/4^d, и дробные границы законны: сэмплирование по ним
/// и есть то усреднение, которое дал бы точный тайл.
pub fn cell_uv(cell: [f64; 4], carrier: (u32, u32, u32), tex_w: u32, tex_h: u32) -> [f32; 4] {
    let (level, x, y) = carrier;
    let px = level_px(level);
    let origin_x = f64::from(x) * f64::from(TILE);
    let origin_y = f64::from(y) * f64::from(TILE);
    let u = |ix: f64| ((ix / px - origin_x) / f64::from(tex_w)).clamp(0.0, 1.0) as f32;
    let v = |iy: f64| ((iy / px - origin_y) / f64::from(tex_h)).clamp(0.0, 1.0) as f32;
    [u(cell[0]), v(cell[1]), u(cell[2]), v(cell[3])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_size_is_iterated_halving() {
        // Композиция округлений: два деления пополам = одно на четыре.
        for side in [1u32, 2, 3, 511, 512, 513, 1000, 10980] {
            for level in 0..8 {
                assert_eq!(level_size(side, level), side.div_ceil(1 << level));
            }
        }
        assert_eq!(level_size(10980, 0), 10980);
        assert_eq!(level_size(10980, 1), 5490);
        assert_eq!(level_size(10980, 5), 344);
    }

    #[test]
    fn level_count_boundaries() {
        // Помещается в тайл — уровень один; на пиксель больше — уже два.
        assert_eq!(level_count(TILE, TILE), 1);
        assert_eq!(level_count(TILE + 1, 1), 2);
        assert_eq!(level_count(1, 1), 1);
        assert_eq!(level_count(0, 100), 0);
        // TCI Sentinel-2: 10980 → 5490 → 2745 → 1373 → 687 → 344.
        assert_eq!(level_count(10980, 10980), 6);
        // Вершина действительно помещается в один тайл.
        for (w, h) in [(10980, 10980), (7000, 3), (513, 513)] {
            let top = level_count(w, h) - 1;
            assert!(level_size(w, top) <= TILE && level_size(h, top) <= TILE);
        }
    }

    #[test]
    fn grid_and_extent_cover_side_exactly() {
        for side in [1u32, TILE - 1, TILE, TILE + 1, 3 * TILE, 10980] {
            let tiles = grid(side);
            let covered: u32 = (0..tiles).map(|i| tile_extent(i, side)).sum();
            assert_eq!(covered, side, "сторона {}", side);
            // Все тайлы, кроме последнего, полные.
            for i in 0..tiles.saturating_sub(1) {
                assert_eq!(tile_extent(i, side), TILE);
            }
            assert_eq!(tile_extent(tiles, side), 0);
        }
    }

    #[test]
    fn exact_tile_uses_whole_texture() {
        let cell = cell_image_rect(1, 0, 0, 1024, 1024);
        assert_eq!(cell, [512.0, 0.0, 1024.0, 512.0]);
        assert_eq!(cell_uv(cell, (0, 1, 0), 512, 512), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn parent_serves_its_quadrant() {
        // Ячейка (0; 1,1) — правый нижний квадрант тайла-предка (1; 0,0).
        let cell = cell_image_rect(1, 1, 0, 2048, 2048);
        assert_eq!(cell_uv(cell, (1, 0, 0), 512, 512), [0.5, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn edge_cell_is_clipped_by_image() {
        // 700 px: уровень 0 — ячейки 0..2, правая короче; краевой тайл
        // 188×40 используется целиком.
        let cell = cell_image_rect(1, 0, 0, 700, 40);
        assert_eq!(cell, [512.0, 0.0, 700.0, 40.0]);
        assert_eq!(cell_uv(cell, (0, 1, 0), 188, 40), [0.0, 0.0, 1.0, 1.0]);
    }
}
