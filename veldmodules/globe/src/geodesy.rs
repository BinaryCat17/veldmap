use glam::{DVec3, Vec3};

pub const SEMI_MAJOR_M: f64 = 6_378_137.0;

pub(crate) const FLATTENING: f64 = 1.0 / 298.257_223_563;

const ECCENTRICITY_SQ: f64 = FLATTENING * (2.0 - FLATTENING);

pub type World = [f32; 3];

#[derive(Clone, Copy, PartialEq)]
pub struct Geodetic {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub height_m: f64,
}

impl Geodetic {
    pub fn surface(lat_deg: f64, lon_deg: f64) -> Self {
        Self { lat_deg, lon_deg, height_m: 0.0 }
    }
}

pub fn world(point: Geodetic) -> DVec3 {
    let (sin_lon, cos_lon) = point.lon_deg.to_radians().sin_cos();
    parallel(point.lat_deg, point.height_m).at(sin_lon, cos_lon)
}

#[derive(Clone, Copy)]
pub struct Parallel {
    sin_lat: f64,
    cos_lat: f64,
    equatorial: f64,
    polar: f64,
}

pub fn parallel(lat_deg: f64, height_m: f64) -> Parallel {
    let (sin_lat, cos_lat) = lat_deg.to_radians().sin_cos();

    let curvature = SEMI_MAJOR_M / (1.0 - ECCENTRICITY_SQ * sin_lat * sin_lat).sqrt();

    Parallel {
        sin_lat,
        cos_lat,
        equatorial: (curvature + height_m) / SEMI_MAJOR_M,
        polar: (curvature * (1.0 - ECCENTRICITY_SQ) + height_m) / SEMI_MAJOR_M,
    }
}

impl Parallel {
    pub fn at(&self, sin_lon: f64, cos_lon: f64) -> DVec3 {
        DVec3::new(self.cos_lat * cos_lon, self.cos_lat * sin_lon, self.sin_lat)
            * DVec3::new(self.equatorial, self.equatorial, self.polar)
    }
}

pub fn position(point: Geodetic) -> World {
    world(point).as_vec3().to_array()
}

pub fn parts(point: DVec3) -> (World, World) {
    let high = point.as_vec3();
    (high.to_array(), (point - high.as_dvec3()).as_vec3().to_array())
}

pub fn offset(head: (World, World), tail: (World, World)) -> World {
    let high = Vec3::from(head.0) - Vec3::from(tail.0);
    let low = Vec3::from(head.1) - Vec3::from(tail.1);
    (high + low).to_array()
}

pub fn intersect_at(origin: DVec3, direction: DVec3, height_m: f64) -> Option<DVec3> {
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

    let t = (-b - discriminant.sqrt()) / (2.0 * a);
    (t > 0.0).then(|| origin + direction * t)
}

pub fn surface_at(point: DVec3) -> (f64, f64) {
    let lat = (point.z / ((1.0 - ECCENTRICITY_SQ) * point.x.hypot(point.y))).atan();
    (lat.to_degrees(), point.y.atan2(point.x).to_degrees())
}

pub fn separation(from: (f64, f64), to: (f64, f64)) -> f64 {
    unit(from.0, from.1).dot(unit(to.0, to.1)).clamp(-1.0, 1.0).acos().to_degrees()
}

pub fn between(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return (to.0, unwind(from.1, to.1));
    }

    let (a, b) = (unit(from.0, from.1), unit(to.0, to.1));
    let angle = a.dot(b).clamp(-1.0, 1.0).acos();
    let sin = angle.sin();
    if sin.abs() < 1e-12 {
        return from;
    }

    let (head, tail) = (((1.0 - t) * angle).sin() / sin, (t * angle).sin() / sin);
    let mid = (a * head + b * tail).normalize();
    (mid.z.asin().to_degrees(), unwind(from.1, mid.y.atan2(mid.x).to_degrees()))
}

pub const FULL_CIRCLE_DEG: f64 = 359.0;

pub const TURN_DEG: f64 = 360.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Along {
    Parallel,
    Meridian,
    Arc,
}

fn along(from: (f64, f64), to: (f64, f64)) -> Along {
    match (from.0 == to.0, from.1 == to.1) {
        (true, _) => Along::Parallel,
        (_, true) => Along::Meridian,
        _ => Along::Arc,
    }
}

pub fn sweep(from: (f64, f64), to: (f64, f64)) -> f64 {
    match (to.1 - from.1).abs() >= FULL_CIRCLE_DEG {
        true => to.1 - from.1,
        false => unwind(from.1, to.1) - from.1,
    }
}

pub fn edge_span(from: (f64, f64), to: (f64, f64)) -> f64 {
    match along(from, to) {
        Along::Parallel => sweep(from, to).abs() * from.0.to_radians().cos(),
        Along::Meridian => (to.0 - from.0).abs(),
        Along::Arc => separation(from, to),
    }
}

pub fn edge_point(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    match along(from, to) {
        Along::Parallel => (from.0, from.1 + sweep(from, to) * t),
        Along::Meridian => (from.0 + (to.0 - from.0) * t, from.1),
        Along::Arc => between(from, to, t),
    }
}

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

pub fn unwind(base: f64, lon_deg: f64) -> f64 {
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

pub fn normal(lat_deg: f64, lon_deg: f64) -> World {
    unit(lat_deg, lon_deg).as_vec3().to_array()
}

pub fn metres(value: f64) -> f32 {
    (value / SEMI_MAJOR_M) as f32
}

pub fn angles(unit: DVec3) -> (f64, f64) {
    (unit.z.clamp(-1.0, 1.0).asin().to_degrees(), unit.y.atan2(unit.x).to_degrees())
}

pub fn unit(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (sin_lat, cos_lat) = lat_deg.to_radians().sin_cos();
    let (sin_lon, cos_lon) = lon_deg.to_radians().sin_cos();
    DVec3::new(cos_lat * cos_lon, cos_lat * sin_lon, sin_lat)
}
