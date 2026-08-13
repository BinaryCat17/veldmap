//! Одна канва: что в ней показано, куда смотрит камера и как из тайлов
//! собирается кадр.
//!
//! Ячейка кадра — тайл текущего уровня. Рисуется она ровно одним квадом:
//! точным тайлом, а пока его нет — куском ближайшего имеющегося предка
//! (адресация пирамиды это позволяет: тайл уровня z+1 накрывает четыре тайла
//! уровня z, см. resample::halve у тайлера). Одна ячейка — один квад, поэтому
//! перекрытий нет и буфер глубины не нужен.

use std::collections::HashSet;

use veldmap_image_tiler_wrap::pyramid::{self, TILE};
use veldsdk::graphics::GrowingBuffer;

use super::camera::Camera;
use super::gpu::{self, Quad, Target};
use super::tiles::{Addr, TileStore};

pub struct View {
    pub label: String,
    pub target: Option<Target>,
    pub shown: Option<Shown>,
    /// Камера появляется первым вписыванием — когда известны и снимок, и
    /// размер канвы — и дальше живёт своей жизнью.
    pub camera: Option<Camera>,
    pub vertices: GrowingBuffer,
    /// Из чего собран последний записанный кадр; совпало — кадр пропускается.
    pub drawn: Option<Stamp>,
    /// Тайлы, за которыми уже послано — в кэш или производителю. Второй раз
    /// не просим: ответ либо едет, либо придёт терминалом с промахами.
    pub inflight: HashSet<Addr>,
    /// Тайлы, которые произвести не удалось. Не переспрашиваются до нового
    /// показа — иначе каждый сдвиг камеры долбил бы производителя тем же
    /// отказом.
    pub failed: HashSet<Addr>,
    /// Активное производство: корреляция (ею операция и убивается) и уровень.
    pub produce: Option<(String, u32)>,
    /// Актуальность описания: показ могли сменить, пока ответ шёл.
    pub describe: veldsdk::Latest,
    pub read_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
    /// Последнее разосланное состояние — дедуп on_view_state.
    pub reported: Option<crate::proto::image_view::ViewState>,
}

/// Показанный ресурс. Метаданные приходят вторым тактом (on_described),
/// и до них снимок «есть, но рисовать нечего».
pub struct Shown {
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
}

pub struct Meta {
    pub fingerprint: String,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
}

/// То, из чего собран кадр. Поколение хранилища тайлов заменяет сравнение
/// самих наборов: любой пришедший или вытесненный тайл его двигает.
#[derive(Clone, Copy, PartialEq)]
pub struct Stamp {
    pub camera: Camera,
    pub generation: u64,
    pub texture: u64,
}

impl View {
    pub fn new(label: String) -> Self {
        Self {
            label,
            target: None,
            shown: None,
            camera: None,
            vertices: gpu::vertex_buffer(),
            drawn: None,
            inflight: HashSet::new(),
            failed: HashSet::new(),
            produce: None,
            describe: veldsdk::Latest::default(),
            read_bytes: 0,
            total_bytes: 0,
            error: None,
            reported: None,
        }
    }

    pub fn meta(&self) -> Option<&Meta> {
        self.shown.as_ref()?.meta.as_ref()
    }

    /// Показ идёт: описание в пути или производство не кончилось.
    pub fn busy(&self) -> bool {
        self.describe.is_pending() || self.produce.is_some()
    }
}

/// Видимые ячейки: уровень под текущий масштаб и адреса тайлов, накрывающих
/// видимый прямоугольник. `None` — рисовать пока не из чего (нет места,
/// снимка или камеры).
pub fn desired_cells(view: &View) -> Option<(u32, Vec<Addr>)> {
    let (target, camera, meta) = parts(view)?;
    let level = camera.level(meta.levels);
    let rect = camera.visible((meta.width, meta.height), (target.width, target.height));
    if rect.0 >= rect.2 || rect.1 >= rect.3 {
        return Some((level, Vec::new()));
    }

    let (xs, ys) = cell_range(rect, level, meta);
    let mut cells = Vec::new();
    for y in ys {
        for x in xs.clone() {
            cells.push((level, x, y));
        }
    }
    Some((level, cells))
}

/// Квады кадра: по одному на видимую ячейку, у которой нашёлся хоть какой-то
/// тайл. Обращения продлевают тайлам жизнь в бюджете хранилища.
pub fn quads(view: &View, store: &mut TileStore) -> Vec<Quad> {
    let Some((target, camera, meta)) = parts(view) else { return Vec::new() };
    let Some((level, cells)) = desired_cells(view) else { return Vec::new() };

    let mut quads = Vec::with_capacity(cells.len());
    for (_, x, y) in cells {
        // Ближайший имеющийся предок, начиная с точного тайла.
        for d in 0..=(meta.levels - 1 - level) {
            let addr = (level + d, x >> d, y >> d);
            let Some(stored) = store.touch(&meta.fingerprint, addr) else { continue };
            let cell = pyramid::cell_image_rect(x, y, level, meta.width, meta.height);
            quads.push(Quad {
                rect: to_ndc(cell, camera, target),
                uv: pyramid::cell_uv(cell, addr, stored.width, stored.height),
                bind: stored.bind.clone(),
            });
            break;
        }
    }
    quads
}

fn parts(view: &View) -> Option<(&Target, &Camera, &Meta)> {
    Some((view.target.as_ref()?, view.camera.as_ref()?, view.meta()?))
}

/// Диапазоны ячеек уровня, накрытые прямоугольником снимка. Сама арифметика
/// ячейки (прямоугольник, носитель, UV) — в общем pyramid.rs: ею же считает
/// глобус, и правила у них расходиться не могут.
fn cell_range(
    rect: (f64, f64, f64, f64),
    level: u32,
    meta: &Meta,
) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let cell = f64::from(TILE) * pyramid::level_px(level);
    let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
    let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
    let x0 = ((rect.0 / cell).floor() as u32).min(grid_w);
    let x1 = ((rect.2 / cell).ceil() as u32).min(grid_w);
    let y0 = ((rect.1 / cell).floor() as u32).min(grid_h);
    let y1 = ((rect.3 / cell).ceil() as u32).min(grid_h);
    (x0..x1, y0..y1)
}

/// Пиксели снимка → NDC кадра. Y переворачивается: у снимка вниз, у NDC вверх.
fn to_ndc(cell: [f64; 4], camera: &Camera, target: &Target) -> [f32; 4] {
    let (w, h) = (f64::from(target.width), f64::from(target.height));
    let sx = |ix: f64| (ix - camera.cx) * camera.scale + w / 2.0;
    let sy = |iy: f64| (iy - camera.cy) * camera.scale + h / 2.0;
    [
        (sx(cell[0]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[1]) / h * 2.0) as f32,
        (sx(cell[2]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[3]) / h * 2.0) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(width: u32, height: u32) -> Meta {
        Meta {
            fingerprint: "t".into(),
            width,
            height,
            levels: pyramid::level_count(width, height),
        }
    }

    #[test]
    fn cell_range_covers_visible_rect_exactly() {
        let meta = meta(10980, 10980);
        // Весь снимок на уровне 5 (344 px) — одна ячейка.
        let (xs, ys) = cell_range((0.0, 0.0, 10980.0, 10980.0), 5, &meta);
        assert_eq!((xs, ys), (0..1, 0..1));
        // Кусок нулевого уровня: пиксели 500..1300 → ячейки 0..3.
        let (xs, _) = cell_range((500.0, 0.0, 1300.0, 1.0), 0, &meta);
        assert_eq!(xs, 0..3);
    }
}
