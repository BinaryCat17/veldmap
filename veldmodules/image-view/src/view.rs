//! Одна канва: что в ней показано, куда смотрит камера и как из тайлов
//! собирается кадр.
//!
//! Ячейка кадра — тайл текущего уровня. Рисуется она ровно одним квадом:
//! точным тайлом, а пока его нет — куском ближайшего имеющегося предка
//! (адресация пирамиды это позволяет: тайл уровня z+1 накрывает четыре тайла
//! уровня z, см. resample::halve у тайлера). Одна ячейка — один квад, поэтому
//! перекрытий нет и буфер глубины не нужен.

use veldmap_image_tiler_wrap::pyramid::{self, TILE};
use veldmap_image_tiler_wrap::tiles::{self, Fetch, Meta, Passes, Store};
use veldsdk::graphics::GrowingBuffer;

use super::camera::Camera;
use super::gpu::{self, Quad, Target};

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
    /// Что уже спрошено, что производится и чего больше не просить — общий
    /// учёт потребителя тайлов (см. `veldmap_image_tiler_wrap::tiles`).
    pub fetch: Fetch,
    /// Актуальность описания: показ могли сменить, пока ответ шёл.
    pub describe: veldsdk::Latest,
    pub read_bytes: u64,
    pub total_bytes: u64,
    /// Смотреть не на что: ресурс не открылся или не описался.
    pub error: Option<String>,
    /// Кадр неполон: сорвался проход, отказал кэш. Снимок при этом жив, и
    /// показывать причину вместо него было бы неправдой — а держать её вечно
    /// незачем: снимается первым же приехавшим тайлом (см. [`View::landed`]).
    ///
    /// Слово о причине говорится здесь целиком: на провод обе жалобы уезжают
    /// одним полем, и заказчик показывает сказанное как есть.
    pub trouble: Option<String>,
    /// Кадр не записался вовсе — это жалоба на сам рендер, а не на конвейер.
    /// Врозь с `trouble` затем, что снимают их разные события: конвейерную
    /// снимает приехавший тайл, эту — удавшийся кадр. Одним полем они гасили
    /// бы друг друга: успешная перерисовка дыры стирала бы «не доехало», а
    /// приехавший тайл — «кадр застыл», который никуда не делся.
    pub stuck: Option<String>,
}

/// Показанный ресурс. Метаданные приходят вторым тактом (on_described),
/// и до них снимок «есть, но рисовать нечего».
pub struct Shown {
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
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
            fetch: Fetch::default(),
            describe: veldsdk::Latest::default(),
            read_bytes: 0,
            total_bytes: 0,
            error: None,
            trouble: None,
            stuck: None,
        }
    }

    pub fn meta(&self) -> Option<&Meta> {
        self.shown.as_ref()?.meta.as_ref()
    }

    /// Тайл лёг — жалоба на прошлое снимается. Картинка достраивается прямо
    /// сейчас, и подпись «не доехало» рядом с ней говорила бы о том, чего уже
    /// нет.
    pub fn landed(&mut self) {
        self.trouble = None;
    }

    /// Показ идёт: описание в пути либо по пирамиде ещё есть работа. Второе
    /// считает общее правило (`tiles::working`) — то же, по которому отвечает
    /// на этот вопрос наложение на шаре.
    pub fn working<K: PartialEq>(&self, passes: &Passes<K>, want: Option<&tiles::Want>) -> bool {
        self.describe.is_pending()
            || self.meta().is_some_and(|meta| {
                tiles::working(&self.fetch, passes, &meta.fingerprint, want)
            })
    }
}

/// Что нужно канве прямо сейчас: цель, ступень к ней и её ячейки. `None` —
/// рисовать пока не из чего (нет места, снимка или камеры).
///
/// Считает это общее правило (`tiles::want`) — то же, по которому выбирает
/// уровень наложение на шаре. Канва приносит туда одно своё: уровень под
/// масштаб камеры и ячейки, накрывающие видимый прямоугольник.
///
/// Отвечает оно обоим — и запросу тайлов, и сборке кадра: рисовать не то, что
/// просили, значит либо просить невидимое, либо не показывать добытое.
pub fn wanted(view: &View, store: &Store, cap: u64) -> Option<tiles::Want> {
    let (target, camera, meta) = parts(view)?;
    let rect = camera.visible((meta.width, meta.height), (target.width, target.height));
    let empty = rect.0 >= rect.2 || rect.1 >= rect.3;

    Some(tiles::want(
        camera.level(meta.levels),
        meta.levels,
        meta.finest,
        cap,
        meta.reach,
        store,
        &view.fetch,
        &meta.fingerprint,
        |level| match empty {
            true => Vec::new(),
            false => {
                let (xs, ys) = cell_range(rect, level, meta);
                ys.flat_map(|y| xs.clone().map(move |x| (level, x, y))).collect()
            }
        },
    ))
}

/// Квады кадра: по одному на видимую ячейку, у которой нашёлся носитель —
/// точный тайл либо ближайший имеющийся предок (`Store::carrier`, общий с
/// наложениями). Обращения продлевают тайлам жизнь в бюджете хранилища.
pub fn quads(view: &View, store: &mut Store, cap: u64) -> Vec<Quad> {
    let Some((target, camera, meta)) = parts(view) else { return Vec::new() };
    let Some(want) = wanted(view, store, cap) else { return Vec::new() };

    let mut quads = Vec::with_capacity(want.cells.len());
    for cell in want.cells {
        let Some((addr, stored)) = store.carrier(&meta.fingerprint, cell, meta.levels) else {
            continue;
        };
        let (bind, tex) = (stored.bind.clone(), (stored.width, stored.height));
        let rect = pyramid::cell_image_rect(cell.1, cell.2, want.level, meta.width, meta.height);
        quads.push(Quad {
            rect: to_ndc(rect, camera, target),
            uv: pyramid::cell_uv(rect, addr, tex.0, tex.1),
            bind,
        });
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
            reach: crate::proto::image_tiler::Reach::Exact,
            finest: 0,
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
