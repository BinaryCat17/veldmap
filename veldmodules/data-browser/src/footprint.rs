//! Contour of a snapshot: where it lies and what falls inside it.
//!
//! Two tasks, and both about place, not drawing: aim the camera at the snapshot
//! and tell whether a click landed on it. The drawing module cannot be entrusted
//! with them — it knows nothing of snapshots at all; the provider even less — it
//! hands over geometry and that is all. Only the one who has both the list and
//! the globe on screen can put one with the other.
//!
//! Counted on the sphere, not on a pair of angles and not on the ellipsoid. Not
//! on the ellipsoid — because flattening adds fractions of a percent, while
//! these numbers serve to pull away from the snapshot to a distance from which
//! it is seen whole, and to tell whether it was hit or missed.
//!
//! The arithmetic itself is not its own: edge tracing, the unit vector, the
//! inverse path back to angles, and longitude unwinding all come from
//! `veldmap_globe_wrap::geodesy` — the same file the globe draws with. The one
//! thing of its own here is the azimuth: it is needed to count turns and
//! nowhere else.
//!
//! Shared not to save lines but for one answer to two questions. The contour
//! edge is traced in three different ways (`geodesy::along`): an imaging swath
//! by an arc, a raster on a degree grid by a parallel or a meridian. Were
//! "inside" counted from the vertices, all edges would come out as arcs, and an
//! arc between the ends of a parallel bows toward the pole the farther the
//! longer the edge is in degrees of longitude: a strip along the top edge would
//! be picked by a click while drawn outside, and along the bottom edge the other
//! way around. So the ring here is traced with the same walk as it is drawn.
//!
//! The agreement is not exact, and it cannot be promised: turns are counted
//! along the polyline inscribed in that walk, and a polyline inscribed in a
//! parallel passes north of it. The remainder is the polyline step (see
//! [`traced`]): it is much smaller than that arc, and grows toward the pole.
//! Only counting along the parallel itself instead of the chord would make it
//! exact.
//!
//! A pair of angles is not enough, and that is no quibble: latitude with
//! longitude has a pole and has a seam. A Sentinel-1 polar granule skirts the
//! cap with a ring whose vertices all lie south of any point inside it —
//! intersection counting along latitude answers "outside" for such a ring, and
//! averaging raw longitudes puts its centre on the opposite side of the Earth.
//! Unit vectors know neither failure.

use glam::DVec3;
use veldmap_globe_wrap::geodesy;

use crate::proto::data_provider::Ring;

/// The circle the contour is inscribed in.
#[derive(Clone, Copy)]
pub struct Frame {
    pub lat: f64,
    pub lon: f64,
    /// Angular radius in degrees of arc: 1° is about 111 km.
    pub radius_deg: f64,
}

/// Where to look to see these rings whole. `None` — there are no rings: the
/// snapshot had no geometry, and there is nothing to aim the camera at.
pub fn frame(rings: &[Ring]) -> Option<Frame> {
    let first = rings.iter().flat_map(|ring| &ring.points).next()?;

    // Vertices spread over the whole sphere and cancelled each other out — such
    // a set has no middle. Take the first vertex: the radius comes out larger,
    // and the snapshot fits into the frame all the same.
    let (lat, lon) = middle(points(rings)).unwrap_or((first.lat, first.lon));

    // Radius to the farthest vertex, not half the bounding box: for a contour
    // stretched along a diagonal, the box is half of what failed to fit.
    let radius_deg = points(rings)
        .map(|point| geodesy::separation((lat, lon), point))
        .fold(0.0, f64::max);

    Some(Frame { lat, lon: wrap(lon), radius_deg })
}

/// Do the rings cover this point.
///
/// Polygon holes arrive as rings on equal footing with the outer contour, and
/// here it goes unnoticed: a point in a hole lies inside both, and the question
/// is "did the click land on the snapshot", whose answer a hole does not
/// change.
pub fn covers(rings: &[Ring], lat: f64, lon: f64) -> bool {
    rings.iter().any(|ring| inside(&angles(ring), lat, lon))
}

/// All vertices of the ring set as pairs of angles.
fn points(rings: &[Ring]) -> impl Iterator<Item = (f64, f64)> + '_ {
    rings.iter().flat_map(|ring| &ring.points).map(|point| (point.lat, point.lon))
}

/// Ring as pairs of angles — in the form `geodesy` understands it.
fn angles(ring: &Ring) -> Vec<(f64, f64)> {
    ring.points.iter().map(|point| (point.lat, point.lon)).collect()
}

/// Ring densified with the same walk as it is drawn.
///
/// The step here is needed not for line smoothness but so that the turn count
/// follows the same curve — and its measure is therefore not pixels but metres
/// of miss: a polyline inscribed in a parallel departs from it by
/// `R·s²/(8·cos φ)`, and a quarter degree leaves fifty metres at seventies
/// latitudes against three kilometres for two degrees. A fifty-metre miss is
/// less than a pixel at any zoom from which the snapshot is seen whole.
///
/// The value is its own, not borrowed from the drawer: there the step derives
/// from the height above the surface, here from metres on the ground, and the
/// agreement of their magnitudes (the drawer divides finer than a third of a
/// degree) is a consequence, not a rule.
///
/// The measure of the step is the edge length along the surface
/// (`geodesy::edge_span`), so for a parallel it grows with longitude toward the
/// pole, and with it grows the remainder of the disagreement (see the header).
fn traced(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    const MAX_EDGE_DEG: f64 = 0.25;

    let mut out = Vec::with_capacity(ring.len());
    for (at, &from) in ring.iter().enumerate() {
        let to = ring[(at + 1) % ring.len()];
        // No more steps than an edge needs for the full circle: an edge is
        // never longer, and the `u32` cast saturates instead of overflowing —
        // from a number that arrived over the bus, four billion steps would
        // come out.
        //
        // The bound applies to the steps, not the span: `f64::min` lets a
        // non-number through as the second argument, and a span clamped by it
        // would give a full circle where there is nothing to trace at all.
        const MOST: f64 = geodesy::TURN_DEG / MAX_EDGE_DEG;
        let steps = (geodesy::edge_span(from, to) / MAX_EDGE_DEG).ceil().max(1.0).min(MOST) as u32;
        for step in 0..steps {
            out.push(geodesy::edge_point(from, to, f64::from(step) / f64::from(steps)));
        }
    }
    out
}

/// Does the point lie inside the ring.
///
/// Counted by azimuths: look from the point at each vertex in turn and sum the
/// turns of the gaze. A ring that went around the point returns the gaze to its
/// place, turning it exactly one full revolution; a ring off to the side swings
/// back and forth and returns to zero.
///
/// This way, not by crossing parity along latitude: the latter has a pole. A
/// ring skirting the cap lies entirely south of a point inside itself, no edge
/// of it crosses that latitude, and the answer comes out "outside" — which is
/// exactly the polar granule, worth clicking no less than any other. Azimuths
/// have no special point: the pole is to them a place like any other.
///
/// One revolution is not enough at that, and that is no quibble with the
/// formula: on a sphere a closed line has no outer side — it divides the sphere
/// in two, and a ring around the north cap goes round both it and everything
/// else together with the south. The sign tells the sides apart, and which of
/// the two is the snapshot is told by its own middle: there are no snapshots
/// larger than half the Earth.
fn inside(ring: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let traced = traced(ring);
    let turned = turn(&traced, lat, lon);
    if turned.abs() > 180.0 {
        // The middle of the ring is certainly on its side. Degenerate (vertices
        // spread over the whole sphere and cancelled each other out) — there is
        // nothing to name the side with, and the turn itself is what remains.
        let Some((centre_lat, centre_lon)) = middle(ring.iter().copied()) else {
            return true;
        };
        return turned.signum() == turn(&traced, centre_lat, centre_lon).signum();
    }

    // The walk did not converge to anything. For a ring off to the side this
    // does mean "missed", but a belt looks the same: its two full circles go
    // round the sphere in opposite directions and cancel at any point of the
    // planet. What tells them apart is whether a longitude free of the ring
    // remains on the sphere — for a belt none remains, and the question to it
    // is a different one: not "did it go round the point", but "between which
    // parallels does it lie".
    //
    // A polar cap does not come here: its walk honestly converges to a full
    // revolution, and the branch above answers for it.
    geodesy::encircles(&traced) && {
        let (low, high) = ring
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), point| (low.min(point.0), high.max(point.0)));
        (low..=high).contains(&lat)
    }
}

/// By how much the gaze from the point turns when going round the ring.
fn turn(ring: &[(f64, f64)], lat: f64, lon: f64) -> f64 {
    let mut turned = 0.0;
    for (index, from) in ring.iter().enumerate() {
        let to = ring[(index + 1) % ring.len()];
        turned += wrap(bearing(lat, lon, to.0, to.1) - bearing(lat, lon, from.0, from.1));
    }
    turned
}

/// Middle of a point set — the direction of the sum of their unit vectors. No
/// seam and no pole exists for it: points are summed, not angles.
/// `None` — there are no points at all or they cancelled each other out.
fn middle(points: impl Iterator<Item = (f64, f64)>) -> Option<(f64, f64)> {
    let sum: DVec3 = points.map(|(lat, lon)| geodesy::unit(lat, lon)).sum();
    (sum.length() > 1e-9).then(|| geodesy::angles(sum.normalize()))
}

/// Azimuth of the gaze from the first point to the second, degrees. Zero is
/// north.
fn bearing(from_lat: f64, from_lon: f64, to_lat: f64, to_lon: f64) -> f64 {
    let (from, to) = (from_lat.to_radians(), to_lat.to_radians());
    let delta = (to_lon - from_lon).to_radians();
    let east = delta.sin() * to.cos();
    let north = from.cos() * to.sin() - from.sin() * to.cos() * delta.cos();
    east.atan2(north).to_degrees()
}

/// Angle difference wrapped into −180..180 — the same side the eye sees it
/// from.
///
/// The same unwinding as the binding grid, only from zero: there the longitude
/// is unwound toward the neighbouring node, here toward the origin.
fn wrap(degrees: f64) -> f64 {
    geodesy::unwind(0.0, degrees)
}
