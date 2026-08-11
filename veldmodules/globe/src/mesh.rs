//! Геометрия Земли: сама поверхность и сетка параллелей с меридианами.
//!
//! Обе живут в одной паре буферов — вершинном и индексном, — потому что вершина
//! у них одного вида. Разделяет их не буфер, а диапазон индексов и пайплайн,
//! которым его рисуют.
//!
//! Строится всё по узлам географической сетки: широта и долгота идут ровными
//! шагами, а в декартовы координаты их переводит `geodesy`. Из этого следует
//! главное свойство разбиения — растр в EPSG:4326 ложится на него линейно, без
//! пересчёта на каждый пиксель и без искажения у полюсов.

use crate::module::geodesy::{self, Geodetic, World};

/// Вершина: точка поверхности и нормаль к ней. У шара нормаль совпала бы с
/// позицией и второе поле повторяло бы первое — у эллипсоида не совпадает
/// (см. `geodesy::normal`), и хранить приходится обе.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: World,
    pub normal: World,
}

/// Разбиение поверхности, шаг в градусах. Достаточно мелко, чтобы силуэт
/// читался окружностью, и достаточно крупно, чтобы вся геометрия оставалась в
/// одном небольшом буфере.
const LAT_STEP_DEG: f64 = 180.0 / 48.0;
const LON_STEP_DEG: f64 = 360.0 / 96.0;

/// Шаг сетки — 15°, как на школьном глобусе.
const GRID_STEP_DEG: i32 = 15;
/// На сколько отрезков бьётся каждая линия сетки.
const GRID_SEGMENTS: u32 = 96;

/// Высота сетки над поверхностью. Не сдвигом глубины в пайплайне, а
/// геометрией: сдвиг подбирается под точность буфера и разъезжается с ней при
/// смене плоскостей отсечения, а высота — величина с физическим смыслом, и на
/// любой дистанции она одна и та же.
///
/// Два километра: на самом близком подлёте (см. `camera::HEIGHT_RANGE_M`)
/// точность буфера глубины у поверхности — десятки метров, так что этого с
/// запасом хватает, а параллакс с такой высоты ещё не читается.
pub const GRID_HEIGHT_M: f64 = 2_000.0;

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Диапазоны в индексном буфере: поверхность треугольниками, сетка линиями.
    pub surface: std::ops::Range<u32>,
    pub grid: std::ops::Range<u32>,
}

impl Mesh {
    pub fn build() -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let surface_start = indices.len() as u32;
        surface(&mut vertices, &mut indices);
        let surface = surface_start..indices.len() as u32;

        let grid_start = indices.len() as u32;
        graticule(&mut vertices, &mut indices);
        let grid = grid_start..indices.len() as u32;

        Self { vertices, indices, surface, grid }
    }
}

/// Узел поверхности. Общий с контурами: у них та же вершина и та же раскладка
/// буфера — разное только время жизни (см. `outlines`).
pub fn vertex(point: Geodetic) -> Vertex {
    Vertex {
        position: geodesy::position(point),
        normal: geodesy::normal(point.lat_deg, point.lon_deg),
    }
}

/// Поверхность: ряды по широте от полюса к полюсу, в каждом — узлы по долготе.
///
/// Шов на 180° замкнут дублем крайнего столбца вершин, а не переиспользованием
/// нулевого: у совпадающих точек разные долготы, и когда по поверхности поедет
/// растр, его края обязаны разойтись.
fn surface(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    let base = vertices.len() as u32;
    let rows = (180.0 / LAT_STEP_DEG) as u32;
    let columns = (360.0 / LON_STEP_DEG) as u32;
    let stride = columns + 1;

    for row in 0..=rows {
        let lat = 90.0 - LAT_STEP_DEG * row as f64;
        for column in 0..=columns {
            let lon = -180.0 + LON_STEP_DEG * column as f64;
            vertices.push(vertex(Geodetic::surface(lat, lon)));
        }
    }

    for row in 0..rows {
        for column in 0..columns {
            let top = base + row * stride + column;
            let bottom = top + stride;
            indices.extend_from_slice(&[top, bottom, top + 1]);
            indices.extend_from_slice(&[top + 1, bottom, bottom + 1]);
        }
    }
}

/// Меридианы и параллели отрезками.
///
/// Параллели у самых полюсов не рисуются: там они вырождаются в точку, а
/// меридианы и так сходятся.
fn graticule(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    // Ломаная отрезками: каждая пара соседних точек — своя линия. Line strip
    // избавил бы от половины индексов, но потребовал бы отдельного вызова на
    // каждую линию — а их здесь под сорок.
    let mut polyline = |points: Vec<Vertex>| {
        let base = vertices.len() as u32;
        let count = points.len() as u32;
        vertices.extend(points);
        for i in 0..count.saturating_sub(1) {
            indices.extend_from_slice(&[base + i, base + i + 1]);
        }
    };

    let at = |lat: f64, lon: f64| vertex(Geodetic { lat_deg: lat, lon_deg: lon, height_m: GRID_HEIGHT_M });

    for step in 0..(360 / GRID_STEP_DEG) {
        let lon = (step * GRID_STEP_DEG) as f64;
        polyline(
            (0..=GRID_SEGMENTS)
                .map(|i| at(-90.0 + 180.0 * i as f64 / GRID_SEGMENTS as f64, lon))
                .collect(),
        );
    }

    for step in (-90 + GRID_STEP_DEG..90).step_by(GRID_STEP_DEG as usize) {
        let lat = step as f64;
        polyline(
            (0..=GRID_SEGMENTS)
                .map(|i| at(lat, -180.0 + 360.0 * i as f64 / GRID_SEGMENTS as f64))
                .collect(),
        );
    }
}
