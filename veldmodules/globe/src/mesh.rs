use crate::module::geodesy::{self, Geodetic, World};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: World,
    pub position_low: World,
    pub normal: World,
}

const LAT_STEP_DEG: f64 = 180.0 / 48.0;
const LON_STEP_DEG: f64 = 360.0 / 96.0;

const GRID_STEP_DEG: i32 = 15;

const GRID_SEGMENTS: u32 = 672;

pub const SURFACE_LIFT_M: f64 = 80.0;

pub fn drawn_radius() -> f64 {
    1.0 + SURFACE_LIFT_M / geodesy::SEMI_MAJOR_M
}

const CLEARANCE_M: f64 = 10.0;

pub fn sag_m(arc_deg: f64) -> f64 {
    geodesy::SEMI_MAJOR_M * (1.0 - (arc_deg.to_radians() / 2.0).cos())
}

pub fn max_chord_deg() -> f64 {
    let sag = (SURFACE_LIFT_M - CLEARANCE_M) / geodesy::SEMI_MAJOR_M;
    2.0 * (1.0 - sag).clamp(-1.0, 1.0).acos().to_degrees()
}

pub fn max_quad_side_deg() -> f64 {
    max_chord_deg() / std::f64::consts::SQRT_2
}

pub fn lift_m(chord_deg: f64) -> f64 {
    SURFACE_LIFT_M.max(sag_m(chord_deg) + CLEARANCE_M)
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
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

pub fn vertex(point: Geodetic) -> Vertex {
    let (position, position_low) = geodesy::parts(geodesy::world(point));
    Vertex { position, position_low, normal: geodesy::normal(point.lat_deg, point.lon_deg) }
}

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

fn graticule(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    let mut polyline = |points: Vec<Vertex>| {
        let base = vertices.len() as u32;
        let count = points.len() as u32;
        vertices.extend(points);
        for i in 0..count.saturating_sub(1) {
            indices.extend_from_slice(&[base + i, base + i + 1]);
        }
    };

    let height_m = lift_m(360.0 / f64::from(GRID_SEGMENTS));
    let at = |lat: f64, lon: f64| vertex(Geodetic { lat_deg: lat, lon_deg: lon, height_m });

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
