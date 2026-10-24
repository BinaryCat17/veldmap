//! Earth: WGS84 and the single transition from geographic to Cartesian
//! coordinates.
//!
//! Everything that has a place on Earth goes through it: the surface, the grid,
//! the camera, and further the scene footprints. There must not be a second
//! such formula: two that agree by eye will drift apart by tens of kilometres,
//! and that would only be visible by a shifted scene.
//!
//! ## Why the ellipsoid, not the sphere
//!
//! The latitude in the data is **geodetic**: the angle between the normal to
//! the ellipsoid and the equator plane. On a sphere the normal comes out of the
//! centre, and the same latitude would mean a different point: at 45° the
//! divergence reaches 0.19°, which is 21 km along the surface — a fifth of a
//! Sentinel-2 tile missed. The sphere is not simpler than the ellipsoid, it
//! simply lies in the fifth digit, and it lies where it shows.
//!
//! ## Axes
//!
//! Standard ECEF (EPSG:4978); external geometry arrives in the same axes: `X`
//! — toward the intersection of the equator and Greenwich, `Y` — 90° east,
//! `Z` — the north pole. Having a system of our own is deliberately avoided: a
//! foreign point must land where it belongs, without a correction sign
//! somewhere along the way.
//!
//! ## Units
//!
//! Outward — world coordinates in fractions of the semi-major axis: `1.0` at
//! the equator. The scale is uniform along all three axes, so the shape is
//! preserved exactly; stretching the Z axis into a sphere would be exactly the
//! distortion the whole thing is made to avoid.
//!
//! Everything is computed in f64: at the Earth radius an f32 step is about
//! 0.8 m, enough for a vertex in the buffer, but enough neither for
//! intermediate values like the radius of curvature, nor for a ray that is
//! later converted back to degrees. So f32 appears here exactly once, at the
//! very edge — [`position`].
//!
//! The carrier of the triple is `glam::DVec3`, not a home-grown array:
//! component-wise writing by indices 0/1/2 does not let the compiler catch a
//! typo in an index, and that error is silent — a rotation about a mixed-up
//! axis is still a rotation, and a frame orthonormality check passes it.

use glam::{DVec3, Vec3};

/// WGS84 semi-major axis, metres.
pub const SEMI_MAJOR_M: f64 = 6_378_137.0;

/// WGS84 flattening. The polar radius is 21 km shorter than the equatorial —
/// the very 0.34% that make geodetic latitude differ from geocentric.
/// Open to the module's neighbours: the projections (projection.rs) must
/// compute on the same ellipsoid — a second set of constants would silently
/// diverge from this one.
pub(crate) const FLATTENING: f64 = 1.0 / 298.257_223_563;

/// Square of the first eccentricity, `e² = 2f − f²`.
const ECCENTRICITY_SQ: f64 = FLATTENING * (2.0 - FLATTENING);

/// A point in world coordinates — the ones that go into the vertex buffer.
pub type World = [f32; 3];

/// Geographic coordinates. Latitude is geodetic, height is above the ellipsoid
/// (not above sea level: the geoid is unknown here and not needed so far).
#[derive(Clone, Copy, PartialEq)]
pub struct Geodetic {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub height_m: f64,
}

impl Geodetic {
    /// A point on the surface itself.
    pub fn surface(lat_deg: f64, lon_deg: f64) -> Self {
        Self { lat_deg, lon_deg, height_m: 0.0 }
    }
}

/// The ellipsoid point in world coordinates, f64 for those who compute with it
/// further: the vertex buffer is fine with f32, but the ray and the reverse
/// transition to degrees are not (see [`intersect`]).
pub fn world(point: Geodetic) -> DVec3 {
    let (sin_lon, cos_lon) = point.lon_deg.to_radians().sin_cos();
    parallel(point.lat_deg, point.height_m).at(sin_lon, cos_lon)
}

/// Everything at a world point that depends on latitude and height but not on
/// longitude.
///
/// Split out because on a single parallel it does not change, while it costs a
/// trigonometry with a root. The warp mesh of the global raster lies in rows
/// along parallels, and what is computed once per row is shared by all its
/// nodes (see `overlay::nodes_of`).
#[derive(Clone, Copy)]
pub struct Parallel {
    sin_lat: f64,
    cos_lat: f64,
    /// Radii along the equator and along the rotation axis — with height and
    /// already in fractions of the semi-major axis: in them the world goes out
    /// (see the module header). The division happens here and not in
    /// [`Parallel::at`] so that each node gets only multiplications: there
    /// would be three divisions, and they alone would cost more than everything
    /// else in the node.
    equatorial: f64,
    polar: f64,
}

/// Split latitude with height into what the longitudes will then diverge by.
pub fn parallel(lat_deg: f64, height_m: f64) -> Parallel {
    let (sin_lat, cos_lat) = lat_deg.to_radians().sin_cos();

    // Radius of curvature in the prime vertical: the distance from the point
    // to the rotation axis, measured along the normal. This is exactly how the
    // ellipsoid differs from the sphere — on a sphere this value is constant
    // and equals the radius, which is why the latitudes converge.
    let curvature = SEMI_MAJOR_M / (1.0 - ECCENTRICITY_SQ * sin_lat * sin_lat).sqrt();

    // Height is measured along the normal, so it is added to both radii
    // equally, while the radii themselves differ: along the rotation axis the
    // ellipsoid is shorter.
    Parallel {
        sin_lat,
        cos_lat,
        equatorial: (curvature + height_m) / SEMI_MAJOR_M,
        polar: (curvature * (1.0 - ECCENTRICITY_SQ) + height_m) / SEMI_MAJOR_M,
    }
}

impl Parallel {
    /// The point of this parallel at that longitude — its sine with its cosine.
    ///
    /// By longitude split in advance, not by degrees: in a warp-mesh row the
    /// longitudes repeat column after column, and split once they turn a node
    /// into three multiplications.
    pub fn at(&self, sin_lon: f64, cos_lon: f64) -> DVec3 {
        DVec3::new(self.cos_lat * cos_lon, self.cos_lat * sin_lon, self.sin_lat)
            * DVec3::new(self.equatorial, self.equatorial, self.polar)
    }
}

/// The same in the vertex-buffer layout.
pub fn position(point: Geodetic) -> World {
    world(point).as_vec3().to_array()
}

/// A world point split into the high and low halves.
///
/// With a single `f32` a surface point is stored with a step of eighty
/// centimetres: its coordinate is on the order of one, and the step of such a
/// number is two to the minus twenty-third, that is eight tenths of a metre on
/// the semi-major axis. At ten kilometres of height that is a sixth of a pixel,
/// at one kilometre already three pixels, and vertices start to jitter and
/// stick together.
///
/// A pair of halves removes that limit completely: the high one is the same
/// coarse value, the low one is the remainder of it, and together they hold the
/// point itself with steps of hundredths of a micron. This works because **the
/// difference of two close numbers of the same order is exactly representable
/// in `f32`**: the high halves of the vertex and the eye cancel without loss,
/// and the low ones carry the detail.
///
/// The accuracy of the **difference** then comes from the distance to the eye,
/// not from the Earth radius, and it is coarser than the storage accuracy: at a
/// kilometre it is fifty microns, at a hundred metres six. The two are easy to
/// confuse, and they differ by three orders of magnitude.
///
/// Everything drawn rides on such a pair, and it is subtracted in the shader —
/// see `relative` in globe.wgsl.
pub fn parts(point: DVec3) -> (World, World) {
    let high = point.as_vec3();
    (high.to_array(), (point - high.as_dvec3()).as_vec3().to_array())
}

/// The offset from one such point to another.
///
/// Half by half, not by the high halves alone: at neighbouring outline
/// vertices the high halves match to the last digit, and their difference alone
/// would give zero where the neighbour is fifty metres away.
pub fn offset(head: (World, World), tail: (World, World)) -> World {
    let high = Vec3::from(head.0) - Vec3::from(tail.0);
    let low = Vec3::from(head.1) - Vec3::from(tail.1);
    (high + low).to_array()
}

/// Where a ray meets the surface raised by `height_m`. `None` — a miss: past
/// the limb there is no Earth.
///
/// The height is an argument, not a default, and there is deliberately no
/// wrapper with zero here. Asking about the bare ellipsoid is legitimate, but
/// it is asked from here to name the place under the cursor, and what is drawn
/// there is not on the ellipsoid; a second, short door would invite a silent
/// mistake.
///
/// The raised surface is asked about, not the surface itself, because what is
/// seen under the cursor is not the Earth but what is drawn on it — and it is
/// drawn on the lift (`mesh::SURFACE_LIFT_M`). Had the ray intersected the
/// ellipsoid, the answer would name a place standing off the seen one, the more
/// the closer to the frame edge.
///
/// Computed on a sphere: the flattening of the ellipsoid is the compression of
/// a single axis, and by compressing the ray together with it the problem is
/// reduced to a sphere intersection. Otherwise the same quadratic equation
/// would have to be solved with three different coefficients and the same
/// answer.
///
/// A surface raised along the normal does not become a sphere in the compressed
/// space: at the pole it stands off it by `height_m·f/(1−f)`. On the lift that
/// is twenty-seven centimetres — against the hundreds of metres this correction
/// fixes.
///
/// Another consequence of the lift: the sphere grew larger, and right at the
/// limb rays passing by the Earth do hit — a strip `√(2h/R)` wide, about three
/// tenths of a degree, that is thirty kilometres on the ground. There the
/// answer is unreliable anyway: the surface is seen at a grazing angle, and one
/// frame pixel covers those kilometres completely.
pub fn intersect_at(origin: DVec3, direction: DVec3, height_m: f64) -> Option<DVec3> {
    // The polar axis is shorter than the equatorial one by exactly the
    // flattening — stretch the Z axis by the same factor to turn the ellipsoid
    // into a sphere.
    let stretch = DVec3::new(1.0, 1.0, 1.0 / (1.0 - FLATTENING));
    let start = origin * stretch;
    let along = direction * stretch;
    let radius = 1.0 + height_m / SEMI_MAJOR_M;

    let a = along.dot(along);
    let b = 2.0 * start.dot(along);
    let c = start.dot(start) - radius * radius;
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return None;
    }

    // The near root: the far one lies on the back side, which cannot be seen.
    let t = (-b - discriminant.sqrt()) / (2.0 * a);
    (t > 0.0).then(|| origin + direction * t)
}

/// Latitude with longitude of a point lying **on the surface** — the inverse
/// of [`world`].
///
/// Computed for a point on the surface: for one raised above it the latitude
/// separates from the height only by iterations, because the normal along which
/// the height is measured depends on the latitude itself. A point on the lift
/// ([`intersect_at`]) is asked about here too — there the error comes out as
/// `h·e²·sinφ·cosφ/N`, that is the same twenty-seven centimetres, and
/// iterations are not worth starting for it.
pub fn surface_at(point: DVec3) -> (f64, f64) {
    // The ellipsoid normal is steeper than the radius vector by exactly the
    // factor by which the polar axis is compressed — hence the division by
    // (1 − e²). On a sphere the factor would be absent, and both latitudes
    // would coincide.
    let lat = (point.z / ((1.0 - ECCENTRICITY_SQ) * point.x.hypot(point.y))).atan();
    (lat.to_degrees(), point.y.atan2(point.x).to_degrees())
}

/// The angle between two surface points, degrees of arc. 1° is about 111 km.
pub fn separation(from: (f64, f64), to: (f64, f64)) -> f64 {
    unit(from.0, from.1).dot(unit(to.0, to.1)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// A point between two on the surface: the fraction `t` of the way from the
/// first to the second.
///
/// Along the arc, not along the straight line in latitude and longitude, and
/// the difference is not subtle: on the top edge of a Sentinel-1 EW GRDM
/// granule (421 km at 73° N) the arc midpoint stands 13 km off the "in
/// degrees" midpoint. The deviation grows as the square of the edge length.
///
/// A straight line in degrees is a loxodrome, a path of constant azimuth, while
/// the satellite leads the imaging edge by the shortest one. With the four
/// corners of a footprint there is no other way — between them there is nothing
/// but the surface.
///
/// The normals are interpolated: they have the same angles as the coordinates
/// (see [`unit`]), so at the ends the answer is exact, and the longitude stays
/// in the branch of the first point — a contour across the antimeridian is not
/// obliged to break.
pub fn between(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    // The ends are handed out as they are: the return trip through an arcsine
    // would give them only to the last digit, while the joins of neighbouring
    // cells must come out on the very numbers they were computed from.
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return (to.0, unwind(from.1, to.1));
    }

    let (a, b) = (unit(from.0, from.1), unit(to.0, to.1));
    let angle = a.dot(b).clamp(-1.0, 1.0).acos();
    let sin = angle.sin();
    // Coincident points: there is no arc between them, and the sine in the
    // denominator is zero.
    if sin.abs() < 1e-12 {
        return from;
    }

    let (head, tail) = (((1.0 - t) * angle).sin() / sin, (t * angle).sin() / sin);
    let mid = (a * head + b * tail).normalize();
    (mid.z.asin().to_degrees(), unwind(from.1, mid.y.atan2(mid.x).to_degrees()))
}

/// How close a longitude difference must come to the full circle to count as
/// one. The global grid is outlined with edges like −180.0125 → 179.9875, that
/// is, half a pixel short of exact 360°.
pub const FULL_CIRCLE_DEG: f64 = 359.0;

/// One turn of longitude. Unwinds move it by this, and everything that counts
/// branch shifts must stay a multiple of it — to the last digit: a difference
/// of two longitudes is rounded, and fitting it into a turn does not always
/// succeed exactly. The one that counts shifts rounds the quotient itself (see
/// `Grid::unwind`).
///
/// It must not be confused with the tolerance by which a circle is recognised
/// ([`FULL_CIRCLE_DEG`]): that one measures, this one counts.
pub const TURN_DEG: f64 = 360.0;

/// What a contour edge is drawn along.
///
/// The edge itself decides this, and the question here is not about taste. The
/// edge of a scene imaged as a strip runs along the surface by the shortest
/// path — an arc of a great circle. But the edges of a raster lying in a degree
/// grid are exactly parallels and meridians, and the arc between their ends
/// passes by: for an edge along a parallel it cuts toward the pole, and for an
/// edge of a whole 360° it degenerates altogether, because its ends are one and
/// the same point of the sphere. Then of the whole contour of a global product
/// one meridian would remain.
///
/// One is told from the other by the edge itself: a parallel has equal endpoint
/// latitudes, a meridian equal longitudes. A imaging strip has neither: its
/// corners lie wherever they happen to, and a match to the last digit never
/// occurs there.
///
/// It lives here and not with the drawer because two parties ask about it: the
/// drawer (`outlines::edge`) and the one computing the click hit (data-browser,
/// footprint.rs). Were they to disagree, a scene would be outlined in one place
/// and selected in another.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Along {
    Parallel,
    Meridian,
    Arc,
}

/// What the edge between these corners is drawn along.
fn along(from: (f64, f64), to: (f64, f64)) -> Along {
    match (from.0 == to.0, from.1 == to.1) {
        (true, _) => Along::Parallel,
        (_, true) => Along::Meridian,
        _ => Along::Arc,
    }
}

/// How far an edge runs along the longitude.
///
/// For a short side across the antimeridian the arc picks the way itself, while
/// for a parallel it is picked for it, and the pick is one: a full circle stays
/// a circle, everything else unfolds to the nearer branch. A full circle must
/// not be folded — its ends are one and the same point of the sphere, and only
/// a meridian would remain of the contour; and leaving the rest unfolded is
/// even less possible: a box twenty degrees wide across the seam is written as
/// 170 → −170, and read literally it would be outlined around the whole Earth.
pub fn sweep(from: (f64, f64), to: (f64, f64)) -> f64 {
    match (to.1 - from.1).abs() >= FULL_CIRCLE_DEG {
        true => to.1 - from.1,
        false => unwind(from.1, to.1) - from.1,
    }
}

/// Edge length in degrees — a measure of how densely to subdivide it.
pub fn edge_span(from: (f64, f64), to: (f64, f64)) -> f64 {
    match along(from, to) {
        // The length of a parallel is not measured by the longitude difference:
        // at the 70th it is half as short as the equator, and subdividing it
        // twice as densely is pointless.
        Along::Parallel => sweep(from, to).abs() * from.0.to_radians().cos(),
        Along::Meridian => (to.0 - from.0).abs(),
        Along::Arc => separation(from, to),
    }
}

/// A point of the edge at the fraction `t` of the way — by the same rule the
/// edge is drawn by.
pub fn edge_point(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    match along(from, to) {
        Along::Parallel => (from.0, from.1 + sweep(from, to) * t),
        Along::Meridian => (from.0 + (to.0 - from.0) * t, from.1),
        Along::Arc => between(from, to, t),
    }
}

/// Whether the ring goes around the Earth in longitude — that is, whether
/// there is a meridian free of it.
///
/// The **densified** ring is asked about, the one that is drawn: in a belt
/// written with four vertices the full circle sits in a single edge, invisible
/// in the vertices but visible along the way. Hence too the flag does not
/// depend on how many vertices the provider wrote the same line with.
///
/// Computed as the largest gap between neighbouring longitudes: a meridian the
/// ring does not enter is a gap wider than a half-circle, and the sphere can be
/// cut along it. If there is none, the ring has gone around the Earth.
pub fn encircles(ring: &[(f64, f64)]) -> bool {
    if ring.len() < 2 {
        return false;
    }
    let mut lons: Vec<f64> = ring.iter().map(|point| point.1.rem_euclid(360.0)).collect();
    lons.sort_by(f64::total_cmp);

    let wrap = lons[0] + 360.0 - lons[lons.len() - 1];
    let widest = lons
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .fold(wrap, f64::max);
    widest < 180.0
}

/// Longitude into the same branch as `base`: otherwise a set of points on one
/// side of the antimeridian slides half the world apart — interpolation between
/// 179° and −179° would pass through zero.
pub fn unwind(base: f64, lon_deg: f64) -> f64 {
    // How many turns to take off to fit into the half-circle from `base` is
    // computed at once, not by subtracting turn by turn. Not for speed:
    // subtracting turn by turn over an infinite longitude never ends at all —
    // `inf − 360` is still `inf` — and the module would hang on it forever, and
    // a hung host module, unlike a crashed one, is not brought back up.
    //
    // Exactly a half-circle stays where it is, and from both sides: a global
    // raster edge is written as −180 → 180, and shifted to the nearer branch it
    // would collapse the raster into a meridian.
    let apart = lon_deg - base;
    let turns = if apart > 180.0 {
        ((apart - 180.0) / TURN_DEG).ceil()
    } else if apart < -180.0 {
        -((-apart - 180.0) / TURN_DEG).ceil()
    } else {
        0.0
    };
    lon_deg - turns * TURN_DEG
}

/// The unit normal to the ellipsoid — also the local vertical, also what the
/// surface is lit by. On a sphere the position itself would take its place, on
/// an ellipsoid it would not: the direction from the centre deviates from the
/// normal by the same 0.19°.
pub fn normal(lat_deg: f64, lon_deg: f64) -> World {
    unit(lat_deg, lon_deg).as_vec3().to_array()
}

/// Metres into world units — for values that have no place of their own: camera
/// height, grid lift, clipping planes.
pub fn metres(value: f64) -> f32 {
    (value / SEMI_MAJOR_M) as f32
}

/// A pair of angles from a unit vector — the inverse of [`unit`]. A pair, not
/// `Geodetic`: what the vector means is known to the caller, and it has no
/// height.
///
/// There is no pole here: for a direction, unlike for a pair of angles, it is
/// not special in any way — the longitude on it is arbitrary, and
/// `atan2(0, 0)` answers zero, which is no worse than any other answer (see
/// `camera::Camera`).
pub fn angles(unit: DVec3) -> (f64, f64) {
    (unit.z.clamp(-1.0, 1.0).asin().to_degrees(), unit.y.atan2(unit.x).to_degrees())
}

/// A unit vector from a pair of angles. What it means is decided by the caller:
/// with a geodetic latitude it is the normal to the ellipsoid, with a
/// geocentric one the direction from the centre. The formula is one for both,
/// and that is no coincidence — both are spherical, there is nothing here to
/// diverge.
pub fn unit(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (sin_lat, cos_lat) = lat_deg.to_radians().sin_cos();
    let (sin_lon, cos_lon) = lon_deg.to_radians().sin_cos();
    DVec3::new(cos_lat * cos_lon, cos_lat * sin_lon, sin_lat)
}
