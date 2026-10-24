use glam::DVec3;
use veldmap_globe_wrap::geodesy;

use crate::proto::data_provider::Ring;

#[derive(Clone, Copy)]
pub struct Frame {
    pub lat: f64,
    pub lon: f64,
    pub radius_deg: f64,
}

pub fn frame(rings: &[Ring]) -> Option<Frame> {
    let first = rings.iter().flat_map(|ring| &ring.points).next()?;

    let (lat, lon) = middle(points(rings)).unwrap_or((first.lat, first.lon));

    let radius_deg = points(rings)
        .map(|point| geodesy::separation((lat, lon), point))
        .fold(0.0, f64::max);

    Some(Frame { lat, lon: wrap(lon), radius_deg })
}

pub fn covers(rings: &[Ring], lat: f64, lon: f64) -> bool {
    rings.iter().any(|ring| inside(&angles(ring), lat, lon))
}

fn points(rings: &[Ring]) -> impl Iterator<Item = (f64, f64)> + '_ {
    rings.iter().flat_map(|ring| &ring.points).map(|point| (point.lat, point.lon))
}

fn angles(ring: &Ring) -> Vec<(f64, f64)> {
    ring.points.iter().map(|point| (point.lat, point.lon)).collect()
}

fn traced(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    const MAX_EDGE_DEG: f64 = 0.25;

    let mut out = Vec::with_capacity(ring.len());
    for (at, &from) in ring.iter().enumerate() {
        let to = ring[(at + 1) % ring.len()];
        const MOST: f64 = geodesy::TURN_DEG / MAX_EDGE_DEG;
        let steps = (geodesy::edge_span(from, to) / MAX_EDGE_DEG).ceil().max(1.0).min(MOST) as u32;
        for step in 0..steps {
            out.push(geodesy::edge_point(from, to, f64::from(step) / f64::from(steps)));
        }
    }
    out
}

fn inside(ring: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let traced = traced(ring);
    let turned = turn(&traced, lat, lon);
    if turned.abs() > 180.0 {
        let Some((centre_lat, centre_lon)) = middle(ring.iter().copied()) else {
            return true;
        };
        return turned.signum() == turn(&traced, centre_lat, centre_lon).signum();
    }

    geodesy::encircles(&traced) && {
        let (low, high) = ring
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), point| (low.min(point.0), high.max(point.0)));
        (low..=high).contains(&lat)
    }
}

fn turn(ring: &[(f64, f64)], lat: f64, lon: f64) -> f64 {
    let mut turned = 0.0;
    for (index, from) in ring.iter().enumerate() {
        let to = ring[(index + 1) % ring.len()];
        turned += wrap(bearing(lat, lon, to.0, to.1) - bearing(lat, lon, from.0, from.1));
    }
    turned
}

fn middle(points: impl Iterator<Item = (f64, f64)>) -> Option<(f64, f64)> {
    let sum: DVec3 = points.map(|(lat, lon)| geodesy::unit(lat, lon)).sum();
    (sum.length() > 1e-9).then(|| geodesy::angles(sum.normalize()))
}

fn bearing(from_lat: f64, from_lon: f64, to_lat: f64, to_lon: f64) -> f64 {
    let (from, to) = (from_lat.to_radians(), to_lat.to_radians());
    let delta = (to_lon - from_lon).to_radians();
    let east = delta.sin() * to.cos();
    let north = from.cos() * to.sin() - from.sin() * to.cos() * delta.cos();
    east.atan2(north).to_degrees()
}

fn wrap(degrees: f64) -> f64 {
    geodesy::unwind(0.0, degrees)
}
