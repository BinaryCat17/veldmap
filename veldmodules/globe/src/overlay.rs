//! Наложения: снимки, натянутые на поверхность тайлами пирамиды.
//!
//! Наложение — это привязка (рамка UTM либо четырёхугольник футпринта) и до
//! двух растров по ролям: превью даёт картинку сразу одним мелким файлом, к
//! подробному конвейер идёт на приближении. Тайлы добываются тем же танцем,
//! что у канвы превью: describe → query кэшу → produce промахов, — а рисуются
//! патчами варп-сетки: узлы каждой ячейки переводятся привязкой в градусы и
//! дальше геодезией в мир, GPU интерполирует между ними. Растр при этом не
//! ресемплится вовсе — искажение проекции берёт на себя сетка.
//!
//! Ячейка рисуется ровно одним носителем — точным тайлом или куском ближайшего
//! предка (parent-fallback, та же арифметика pyramid.rs, что у канвы), поэтому
//! перекрытий внутри наложения нет.

use std::collections::HashSet;

use veldmap_image_tiler_wrap::pyramid;
use veldsdk::graphics::BindGroupId;

use super::geodesy::{self, Geodetic};
use super::gpu::OverlayVertex;
use super::projection;
use super::tiles::{Addr, TileStore};

/// Высота наложения над поверхностью: над телом Земли (его глубина решает
/// заслонение дальней стороной), но под сеткой и контурами (2 км) — линии
/// обязаны читаться поверх снимка.
pub const HEIGHT_M: f64 = 1_000.0;

/// На сколько сегментов бьётся сторона ячейки варп-сетки. Восьми хватает:
/// даже у ячейки во всю гранулу (110 км) сегмент — 14 км, а хорда такой длины
/// проседает под поверхность на считаные метры против километра высоты.
const PATCH_SEGMENTS: u32 = 8;

/// Роль растра — те же две, что в types.proto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

/// Привязка растра к Земле.
pub enum Frame {
    /// Точная рамка в метрах зоны UTM; y1 — северный край.
    Utm { zone: projection::Zone, x0: f64, y0: f64, x1: f64, y1: f64 },
    /// Четырёхугольник футпринта по обходу растра: UL, UR, LR, LL. Долготы
    /// развёрнуты в непрерывную ветвь заранее (см. [`Frame::quad`]).
    Quad([(f64, f64); 4]),
}

impl Frame {
    /// Квад из вершин по обходу растра. Долготы разворачиваются к первой
    /// вершине: футпринт через антимеридиан иначе интерполировался бы через
    /// всю Землю.
    pub fn quad(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Quad(points.map(|(lat, lon)| {
            let mut lon = lon;
            while lon - base > 180.0 {
                lon -= 360.0;
            }
            while lon - base < -180.0 {
                lon += 360.0;
            }
            (lat, lon)
        }))
    }

    /// Доля растра (0..1 слева направо и сверху вниз) → широта и долгота.
    pub fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        match self {
            Self::Utm { zone, x0, y0, x1, y1 } => {
                let x = x0 + fx * (x1 - x0);
                let y = y1 - fy * (y1 - y0);
                projection::to_geodetic(*zone, x, y)
            }
            Self::Quad([ul, ur, lr, ll]) => {
                let lerp = |a: (f64, f64), b: (f64, f64), t: f64| {
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
                };
                let top = lerp(*ul, *ur, fx);
                let bottom = lerp(*ll, *lr, fx);
                lerp(top, bottom, fy)
            }
        }
    }

    /// Метров земли на пиксель растра шириной `width`. У квада — по верхнему
    /// ребру и грубой метрике градусов: он и так заявленная аппроксимация.
    pub fn ground_m_per_px(&self, width: u32) -> f64 {
        let width = f64::from(width.max(1));
        match self {
            Self::Utm { x0, x1, .. } => (x1 - x0) / width,
            Self::Quad([ul, ur, ..]) => {
                let mid_lat = ((ul.0 + ur.0) * 0.5).to_radians();
                let dlat = (ur.0 - ul.0) * 110_946.0;
                let dlon = (ur.1 - ul.1) * 111_320.0 * mid_lat.cos();
                dlat.hypot(dlon) / width
            }
        }
    }
}

/// Описанный растр — то же, что у канвы.
pub struct Meta {
    pub fingerprint: String,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
}

/// Один растр наложения и все его ожидания.
pub struct Raster {
    pub role: Role,
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
    pub describe: veldsdk::Latest,
    /// Тайлы, за которыми уже послано — в кэш или производителю.
    pub inflight: HashSet<Addr>,
    /// Тайлы, которые произвести не удалось; не переспрашиваются, пока
    /// наложение живо — иначе каждый кадр долбил бы производителя отказом.
    pub failed: HashSet<Addr>,
    /// Активное производство: корреляция (ею оно и убивается) и уровень.
    pub produce: Option<(String, u32)>,
}

impl Raster {
    pub fn new(role: Role, resource: veldsdk::OwnedResource) -> Self {
        Self {
            role,
            resource,
            meta: None,
            describe: veldsdk::Latest::default(),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            produce: None,
        }
    }
}

pub struct Overlay {
    pub key: String,
    pub label: String,
    pub frame: Frame,
    pub rasters: Vec<Raster>,
    /// Ресурсы, которыми наложение прислали, — в том виде, в каком они пришли.
    /// По ним и решается «то же самое наложение или другое»: сравнивать с
    /// принятыми нельзя, потому что принято бывает меньше присланного (растр,
    /// которому отказали в гранте, пропускается), и тогда набор расходился бы
    /// сам с собой навсегда — каждая пересылка выглядела бы новым наложением.
    pub sources: Vec<u64>,
    /// Прозрачность слоя, 0..1. Доезжает до пикселя множителем к альфе
    /// носителя, поэтому прозрачное поле квиклука так и остаётся прозрачным,
    /// а не проступает вполсилы.
    pub opacity: f32,
    /// Слой скрыт: ни патчей, ни запросов тайлов. Уже добытые тайлы остаются в
    /// хранилище на попечение вытеснения — показ обратно тогда мгновенный, а
    /// платить за них памятью приходится только пока их не вытеснили.
    pub hidden: bool,
}

/// Что рисовать у наложения прямо сейчас: какой растр и каким уровнем.
#[derive(Clone, PartialEq)]
pub struct Choice {
    pub role: Role,
    pub fingerprint: String,
    pub level: u32,
}

impl Overlay {
    pub fn raster_mut(&mut self, role: Role) -> Option<&mut Raster> {
        self.rasters.iter_mut().find(|raster| raster.role == role)
    }

    pub fn raster(&self, role: Role) -> Option<&Raster> {
        self.rasters.iter().find(|raster| raster.role == role)
    }

    /// Что рисовать и чего хотеть под текущий взгляд: превью-база и, когда
    /// экран мельче её родного разрешения, подробный растр. Порядок и есть
    /// порядок отрисовки: база всегда внизу, цель поверх — так снимок виден
    /// квиклуком сразу, а подробные тайлы накрывают его по мере прихода
    /// (дешёвый старт из дизайна фазы).
    ///
    /// `mpp_screen` — метров земли на пиксель кадра, `cap_tiles` — потолок
    /// аппетита одного уровня. Уровень каждого растра — ближайший, чей
    /// пиксель не крупнее экранного, но не прожорливее потолка.
    pub fn choices(&self, mpp_screen: f64, cap_tiles: u64) -> Vec<Choice> {
        let mut choices = Vec::new();
        let described =
            |role: Role| self.raster(role).and_then(|raster| raster.meta.as_ref());

        if let Some(meta) = described(Role::Preview) {
            choices.push(self.level_for(Role::Preview, meta, mpp_screen, cap_tiles));
            if mpp_screen >= self.frame.ground_m_per_px(meta.width) {
                // Родного разрешения превью хватает — подробный не нужен.
                return choices;
            }
        }
        if let Some(meta) = described(Role::Detailed) {
            choices.push(self.level_for(Role::Detailed, meta, mpp_screen, cap_tiles));
        }
        choices
    }

    fn level_for(&self, role: Role, meta: &Meta, mpp_screen: f64, cap_tiles: u64) -> Choice {
        let mpp_raster = self.frame.ground_m_per_px(meta.width);
        let mut level = if mpp_screen <= mpp_raster {
            0
        } else {
            (mpp_screen / mpp_raster).log2().floor() as u32
        }
        .min(meta.levels - 1);
        while level < meta.levels - 1 && tiles_at(meta, level) > cap_tiles {
            level += 1;
        }
        Choice { role, fingerprint: meta.fingerprint.clone(), level }
    }
}

/// Тайлов на уровне — аппетит уровня целиком.
fn tiles_at(meta: &Meta, level: u32) -> u64 {
    let w = pyramid::grid(pyramid::level_size(meta.width, level));
    let h = pyramid::grid(pyramid::level_size(meta.height, level));
    u64::from(w) * u64::from(h)
}

/// Патчи наложения: по одному на ячейку выбранного уровня, у которой нашёлся
/// носитель. Вершины пишутся в общий буфер, отрисовки — диапазонами по
/// носителю. Обращения продлевают тайлам жизнь в бюджете.
pub fn patches(
    overlay: &Overlay,
    choice: &Choice,
    store: &mut TileStore,
    vertices: &mut Vec<OverlayVertex>,
    draws: &mut Vec<(BindGroupId, std::ops::Range<u32>)>,
) {
    let Some(meta) = overlay.raster(choice.role).and_then(|raster| raster.meta.as_ref())
    else {
        return;
    };
    let level = choice.level;
    let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
    let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));

    for y in 0..grid_h {
        for x in 0..grid_w {
            // Ближайший имеющийся предок, начиная с точного тайла.
            for d in 0..=(meta.levels - 1 - level) {
                let addr = (level + d, x >> d, y >> d);
                let Some(stored) = store.touch(&meta.fingerprint, addr) else { continue };
                let cell = pyramid::cell_image_rect(x, y, level, meta.width, meta.height);
                let uv = pyramid::cell_uv(cell, addr, stored.width, stored.height);
                let bind = stored.bind.clone();
                let from = vertices.len() as u32;
                patch(&overlay.frame, meta, cell, uv, overlay.opacity, vertices);
                draws.push((bind, from..vertices.len() as u32));
                break;
            }
        }
    }
}

/// Варп-сетка одной ячейки: PATCH_SEGMENTS² квадов, узлы — точной проекцией
/// привязки, UV — линейно по куску носителя.
///
/// Прозрачность едет вершиной, а не uniform'ом: все патчи наложений лежат в
/// одном буфере и рисуются одним пайплайном, отличаясь только диапазоном
/// вершин и bind group носителя, — своего uniform'а у слоя тут нет и не за что
/// его завести.
fn patch(
    frame: &Frame,
    meta: &Meta,
    cell: [f64; 4],
    uv: [f32; 4],
    alpha: f32,
    vertices: &mut Vec<OverlayVertex>,
) {
    let segments = PATCH_SEGMENTS;
    // Узлы решётки считаются один раз: у соседних квадов они общие, а проекция
    // — самая дорогая часть узла.
    let side = (segments + 1) as usize;
    let mut nodes = Vec::with_capacity(side * side);
    for row in 0..=segments {
        let ty = f64::from(row) / f64::from(segments);
        for col in 0..=segments {
            let tx = f64::from(col) / f64::from(segments);
            let fx = (cell[0] + (cell[2] - cell[0]) * tx) / f64::from(meta.width);
            let fy = (cell[1] + (cell[3] - cell[1]) * ty) / f64::from(meta.height);
            let (lat, lon) = frame.geodetic(fx, fy);
            let position =
                geodesy::position(Geodetic { lat_deg: lat, lon_deg: lon, height_m: HEIGHT_M });
            let u = uv[0] + (uv[2] - uv[0]) * tx as f32;
            let v = uv[1] + (uv[3] - uv[1]) * ty as f32;
            nodes.push(OverlayVertex { position, uv: [u, v], alpha });
        }
    }

    for row in 0..segments as usize {
        for col in 0..segments as usize {
            let a = nodes[row * side + col];
            let b = nodes[row * side + col + 1];
            let c = nodes[(row + 1) * side + col];
            let d = nodes[(row + 1) * side + col + 1];
            vertices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(width: u32, height: u32) -> Meta {
        Meta {
            fingerprint: "fp".into(),
            width,
            height,
            levels: pyramid::level_count(width, height),
        }
    }

    fn overlay(rasters: Vec<Raster>) -> Overlay {
        Overlay {
            key: "k".into(),
            label: "снимок".into(),
            // Рамка T40WFC: 109.8 км на всю ширину растра.
            frame: Frame::Utm {
                zone: projection::Zone { number: 40, south: false },
                x0: 600_000.0,
                y0: 7_690_200.0,
                x1: 709_800.0,
                y1: 7_800_000.0,
            },
            rasters,
            sources: Vec::new(),
            opacity: 1.0,
            hidden: false,
        }
    }

    fn raster(role: Role, meta_of: Option<Meta>) -> Raster {
        let mut raster =
            Raster::new(role, veldsdk::OwnedResource::from_raw_id(u64::from(role as u32) + 1));
        raster.meta = meta_of;
        raster
    }

    /// Выбор растров: далеко хватает превью одного; ближе его родного
    /// разрешения подробный добавляется ПОВЕРХ превью — база рисуется всегда,
    /// чтобы снимок был виден, пока подробные тайлы едут.
    #[test]
    fn choices_keep_preview_under_detail() {
        let overlay = overlay(vec![
            raster(Role::Preview, Some(meta(343, 343))),    // ~320 м/px
            raster(Role::Detailed, Some(meta(10980, 10980))), // 10 м/px
        ]);
        // Далеко: экранный пиксель — километр; превью одно, уровень не глубже
        // вершины (у 343² уровень один).
        let far = overlay.choices(1000.0, u64::MAX);
        assert_eq!(far.len(), 1);
        assert_eq!((far[0].role, far[0].level), (Role::Preview, 0));
        // Экран мельче превью: база остаётся, подробный поверх уровнем 2
        // (40 м/px против его 10 м/px).
        let near = overlay.choices(40.0, u64::MAX);
        assert_eq!(near.len(), 2);
        assert_eq!((near[0].role, near[0].level), (Role::Preview, 0));
        assert_eq!((near[1].role, near[1].level), (Role::Detailed, 2));
        // Вплотную: уровень 0 подробного.
        let close = overlay.choices(5.0, u64::MAX);
        assert_eq!((close[1].role, close[1].level), (Role::Detailed, 0));
    }

    /// Потолок аппетита загрубляет уровень: 22×22 тайла нулевого уровня в
    /// потолок из 100 не влезают, 11×11 первого — тоже, 6×6 второго — да.
    #[test]
    fn tile_cap_coarsens_level() {
        let overlay = overlay(vec![raster(Role::Detailed, Some(meta(10980, 10980)))]);
        let choices = overlay.choices(5.0, 100);
        assert_eq!(choices[0].level, 2);
    }

    /// Без описанных растров выбирать не из чего.
    #[test]
    fn no_meta_no_choices() {
        let overlay = overlay(vec![raster(Role::Preview, None)]);
        assert!(overlay.choices(100.0, u64::MAX).is_empty());
    }

    /// Квад разворачивает долготы через антимеридиан в одну ветвь: середина
    /// между 179 и −179 — это 180, а не ноль.
    #[test]
    fn quad_crosses_antimeridian_the_short_way() {
        let frame = Frame::quad([(10.0, 179.0), (10.0, -179.0), (8.0, -179.0), (8.0, 179.0)]);
        let (lat, lon) = frame.geodetic(0.5, 0.0);
        assert!((lat - 10.0).abs() < 1e-12);
        assert!((lon - 180.0).abs() < 1e-9, "{}", lon);
    }

    /// Углы рамки UTM попадают в свои доли растра: (0,0) — северо-западный
    /// угол, (1,1) — юго-восточный, и наоборот через прямую проекцию.
    #[test]
    fn utm_frame_corners_roundtrip() {
        let overlay = overlay(vec![]);
        let (lat_nw, lon_nw) = overlay.frame.geodetic(0.0, 0.0);
        let zone = projection::Zone { number: 40, south: false };
        let (e, n) = projection::from_geodetic(zone, lat_nw, lon_nw);
        assert!((e - 600_000.0).abs() < 1e-6 && (n - 7_800_000.0).abs() < 1e-6);
        let (lat_se, _) = overlay.frame.geodetic(1.0, 1.0);
        assert!(lat_se < lat_nw);
    }
}
