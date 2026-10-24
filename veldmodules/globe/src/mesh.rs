//! Earth geometry: the surface itself and the graticule of parallels and
//! meridians.
//!
//! Both live in one pair of buffers — vertex and index — because their vertex
//! is of one kind. What separates them is not the buffer but a range of indices
//! and the pipeline that draws it.
//!
//! Everything is built on geographic grid nodes: latitude and longitude advance
//! in even steps, and `geodesy` converts them into Cartesian coordinates. Hence
//! the main property of the partitioning — an EPSG:4326 raster maps onto it
//! linearly, with no per-pixel resampling and no distortion near the poles.

use crate::module::geodesy::{self, Geodetic, World};

/// Vertex: a surface point and its normal. On a sphere the normal would coincide
/// with the position and the second field would repeat the first — on an
/// ellipsoid they do not coincide (see `geodesy::normal`), so both must be kept.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    /// High half of the world point; the low half sits beside it. Kept apart
    /// because a single `f32` stores a surface point with an eighty-centimeter
    /// step (see [`geodesy::parts`]).
    pub position: World,
    pub position_low: World,
    pub normal: World,
}

/// Surface partitioning step in degrees. Fine enough for the silhouette to read
/// as a circle, and coarse enough for all the geometry to stay in one buffer:
/// the body by itself hides nothing and competes for nothing — it is inscribed
/// in the ellipsoid and lies below everything drawn on it.
const LAT_STEP_DEG: f64 = 180.0 / 48.0;
const LON_STEP_DEG: f64 = 360.0 / 96.0;

/// Graticule step — 15°, as on a school globe.
const GRID_STEP_DEG: i32 = 15;

/// Into how many segments each graticule line is divided.
///
/// Enough for a segment to fit under the common lift by the same rule as all the
/// others: a parallel at the equator comes out with half-degree segments whose
/// middle sags seventy meters — less than the lift. Divide the graticule any
/// coarser, and its lines would pass under the very surface on which the image
/// and the outline lie, which at the frame edge is hundreds of meters of
/// divergence.
///
/// A multiple of [`LON_STEP_DEG`], so graticule nodes also land on the body's
/// edges and never fight it for depth anywhere but at their own nodes.
///
/// This keeps the buffer from being tiny but still small: all the Earth and
/// graticule geometry is under a megabyte, and it is built once.
const GRID_SEGMENTS: u32 = 672;

/// Lift above the surface — common to everything that lies on it: images,
/// outlines, hatching, the graticule, and the answer to where a click landed.
///
/// Common not for the sake of layer order — order is decided by the draw queue,
/// and only the body of the Earth writes depth (see `gpu::render`). It is common
/// for the sake of screen space: a point at height `L` with the camera at height
/// `H` drifts from the center of the frame by a factor of `H/(H−L)`, so two
/// layers over one and the same place on Earth diverge the closer they are to
/// the frame edge. An outline must lie on its own image rather than beside it,
/// and a click must name the place that is visible under the cursor.
///
/// Spreading the layers over different heights does not buy this: differing
/// heights are the divergence itself. A depth bias in the pipeline does not
/// either: there is no field for it in the contract at all (`graphics.proto`,
/// `DepthStencilState`), and it would have to be tuned to the buffer's precision
/// and drift apart from it whenever the clipping planes change.
///
/// Eighty meters is exactly enough for the sag of the limit segment
/// ([`max_chord_deg`]) together with [`CLEARANCE_M`] to fit underneath. There is
/// no point in taking more: the lift is that very shift on the screen, only
/// common to all; less would mean dividing everything more finely.
///
/// It is bounded by the camera floor, and the tie is written into the floor
/// itself (`camera::HEIGHT_RANGE_M`) rather than left to memory: descending
/// below the lift means passing through what is drawn. The lift introduces no
/// distortion — it is an offset, and everything that measures the camera above
/// the visible measures before it (see [`drawn_radius`]).
pub const SURFACE_LIFT_M: f64 = 80.0;

/// Radius of the drawn surface — in the same fractions of the semi-major axis in
/// which all the geometry of the view is computed.
///
/// What is drawn sits on the lift, not on the ellipsoid, and everything that
/// measures the camera above the **visible** must measure before it: the angular
/// frame size, meters per pixel, the near plane. Measuring to the ellipsoid is
/// wrong by exactly the factor by which the height exceeds the height minus the
/// lift — at ten kilometers that is eight tenths of a percent, at eight hundred
/// meters already eleven.
///
/// Derived here rather than from the lift at each site: otherwise "drawn on the
/// lift" would live in three places separately and diverge on the first edit.
pub fn drawn_radius() -> f64 {
    1.0 + SURFACE_LIFT_M / geodesy::SEMI_MAJOR_M
}

/// Clearance between the sagged middle of a segment and the body of the Earth.
///
/// The body is inscribed in the ellipsoid and can hide what lies above it
/// nowhere except at its own nodes: there it touches the surface. The clearance
/// is for exactly that case, and it is measured by the depth-buffer quantum at
/// the farthest view distance: there it is just under six meters, at ten
/// kilometers a millimeter and a quarter, at eight hundred meters a tenth of a
/// millimeter. Ten meters covers even the worst case with room to spare.
///
/// Position quantization is not part of it: coordinates are counted from the eye
/// in halves, and their step is tens of microns (see `geodesy::parts`).
const CLEARANCE_M: f64 = 10.0;

/// How far the middle of a chord dips below the surface, in meters.
///
/// As a sphere, not an ellipsoid: flattening contributes fractions of a percent
/// to a quantity that is itself a correction.
pub fn sag_m(arc_deg: f64) -> f64 {
    geodesy::SEMI_MAJOR_M * (1.0 - (arc_deg.to_radians() / 2.0).cos())
}

/// Limit chord: an arc whose middle sags exactly enough to fit under the lift
/// along with the clearance. The inverse of [`sag_m`].
pub fn max_chord_deg() -> f64 {
    let sag = (SURFACE_LIFT_M - CLEARANCE_M) / geodesy::SEMI_MAJOR_M;
    2.0 * (1.0 - sag).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Limit side of a quad. Asked for separately because a quad is cut into two
/// triangles and the deepest point is the middle of the hypotenuse — √2 times
/// farther out than the middle of a side.
pub fn max_quad_side_deg() -> f64 {
    max_chord_deg() / std::f64::consts::SQRT_2
}

/// Height at which a carrier with such a longest segment is drawn.
///
/// One that has divided itself finely enough sits on the common lift and
/// converges there with all the others. One that has hit its own density ceiling
/// makes up the remainder with its own rise: an image covering the whole Earth
/// would otherwise be visible in patches near its own grid nodes. It diverges
/// only from those visible along with it, and it is visible only from a distance
/// where the divergence is already below a pixel.
pub fn lift_m(chord_deg: f64) -> f64 {
    SURFACE_LIFT_M.max(sag_m(chord_deg) + CLEARANCE_M)
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Ranges in the index buffer: the surface as triangles, the graticule as lines.
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

/// Surface node. Shared with the outlines: they use the same vertex and the same
/// buffer layout — only the lifetime differs (see `outlines`).
pub fn vertex(point: Geodetic) -> Vertex {
    let (position, position_low) = geodesy::parts(geodesy::world(point));
    Vertex { position, position_low, normal: geodesy::normal(point.lat_deg, point.lon_deg) }
}

/// Surface: rows by latitude from pole to pole, each holding nodes by longitude.
///
/// The seam at 180° is closed with a duplicate of the last vertex column rather
/// than by reusing the first: coincident points have different longitudes, and
/// when a raster travels over the surface, its edges must come apart.
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

/// Meridians and parallels as line segments.
///
/// Parallels nearest the poles are not drawn: there they degenerate into a point,
/// and the meridians converge there anyway.
fn graticule(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    // Polyline as segments: every pair of adjacent points is its own line. A
    // line strip would halve the indices but would need a separate draw call per
    // line — and there are nearly forty of them here.
    let mut polyline = |points: Vec<Vertex>| {
        let base = vertices.len() as u32;
        let count = points.len() as u32;
        vertices.extend(points);
        for i in 0..count.saturating_sub(1) {
            indices.extend_from_slice(&[base + i, base + i + 1]);
        }
    };

    // Height by the same rule as everyone's: the longest graticule segment is
    // the segment of the parallel at the equator.
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
