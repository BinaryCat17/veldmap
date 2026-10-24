//! Outlines on the surface: polylines built from the supplied vertices.
//!
//! A plain outline vertex is the same as the Earth's and the grid's
//! (`mesh::Vertex`) — a point with a normal — so they share one vertex buffer
//! layout. A separate buffer is required not by the format but by lifetime:
//! the Earth is built once, while outlines change with every search response.
//!
//! The selected outline is not a line but a ribbon: a line in the pipeline is
//! always one pixel wide, and one outline among fifty neighbors must stand out
//! clearly. The ribbon width is screen-space, not world-space: in the world it
//! would depend on the zoom — blurring into a blob when far away, thinning back
//! into the same line up close. The vertex shader therefore widens it
//! (`vs_ribbon` in globe.wgsl), and what lives here is what the shader cannot
//! know by itself: the vertex's neighbors along the traversal.
//!
//! A scene still on its way to the globe is hatched over its whole occupied
//! area: the outline says where it will land, and the hatching says how much
//! space it will take. The hatching is built here too, from the same ring — a
//! fan toward the ring's centre.

use glam::DVec3;

use crate::module::geodesy::{self, Geodetic};
use crate::module::gpu::RibbonVertex;
use crate::module::mesh::{self, Vertex};
use crate::proto::globe::{Outline, OutlineStyle};

/// Outline height above the surface — the common lift: outlines are subdivided
/// finely enough to fit on it, and there they meet the scene they enclose.
///
/// The outline has one height for all three styles. Should the line and the
/// fill diverge, the hatching would stick out past its own edge — not because
/// it is built wider, but because it is drawn closer to the eye (see
/// `mesh::SURFACE_LIFT_M`).
fn height_m() -> f64 {
    mesh::lift_m(max_edge_deg() * 2.0)
}

/// How finely an edge is subdivided. A polyline joins vertices with straight
/// segments through space, but it must lie on the surface: the longer the edge,
/// the deeper its segment sinks below that surface.
///
/// Half the limiting chord, not its quotient by √2 as for a uniform grid. The
/// fill fan advances with the same step, and a fan quad is never uniform: at a
/// ring corner the neighboring vertices stand a whole step apart from the ring
/// centre, and that difference adds to the radial step instead of combining
/// with it by the Pythagorean theorem. Hence the factor of two: for a ring
/// leading away from the centre, a quad diagonal comes out twice the step.
fn max_edge_deg() -> f64 {
    mesh::max_chord_deg() / 2.0
}

/// How far from the ring centre its edge may lie for the fill fan to remain
/// buildable — as a cosine of the angle, because it is compared against the dot
/// product of directions.
///
/// The limit is a quarter turn, and it is not a matter of taste: beyond it the
/// ring's points diverge by half the Earth, the ring has no centre at all, and
/// a fan from any point would run over the far side. This is exactly how the
/// climate grid belt is written — from −180 to 180; it gets no fill, but the
/// outline remains, and that is the essential part.
///
/// Five degrees short of the edge are a safety margin: for a ring that reaches
/// exactly a quarter turn, the outer fan nodes converge to a point, and there
/// is nothing left to compute.
const FILL_REACH: f64 = 0.087; // cos 85°

pub struct Outlines {
    /// The outlines themselves — as lines. A scene on its way to the globe is
    /// enclosed by these same lines: what distinguishes it is not the line but
    /// the hatching inside it.
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// The selected one — as a ribbon, with its own pair of buffers: it has its
    /// own vertex and its own pipeline, and there would be nothing to share
    /// with the lines.
    pub ribbon: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
    /// Hatched areas of scenes on their way to the globe. Their vertex is the
    /// same as for the lines and the Earth — a surface point with a normal —
    /// but the buffer pair is their own: they are drawn as triangles, not
    /// lines.
    pub hatch: Vec<Vertex>,
    pub hatch_indices: Vec<u32>,
}

impl Outlines {
    pub fn build(outlines: &[Outline]) -> Self {
        let mut built = Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            ribbon: Vec::new(),
            ribbon_indices: Vec::new(),
            hatch: Vec::new(),
            hatch_indices: Vec::new(),
        };

        for outline in outlines {
            // Densification is one for all three styles: the ribbon, the line
            // and the hatching enclose one and the same outline, and should
            // they diverge by even a vertex — the selected scene would be
            // outlined not where it stood before selection, and the hatching
            // would stick out past its own edge.
            let loops = ring(outline);
            // The fill exists only where a single loop is drawn. Two loops
            // form a belt, and a belt has no centre: a fan from any point would
            // run over exactly what the belt does not cover (see
            // [`FILL_REACH`]). Loop by loop it is even less admissible — a
            // parallel is itself a ring around the pole, and the fan would
            // hatch the cap instead of the belt.
            //
            // It is computed over the loops that survive to rendering: for a
            // rectangle that reaches the pole, the second loop degenerates into
            // a point, while the first is a genuine cap that must be filled.
            let drawn: Vec<&Vec<Vertex>> = loops.iter().filter(|run| run.len() >= 3).collect();
            let whole = drawn.len() == 1;
            for ring in drawn {
                match outline.style() {
                    OutlineStyle::OutlineSelected => built.ribbon(ring),
                    OutlineStyle::OutlinePlain => built.line(ring),
                    // The scene on its way is enclosed by a plain line, with
                    // hatching inside it: what must be said is not "this scene
                    // is special" but "the place is taken".
                    OutlineStyle::OutlinePending => {
                        built.line(ring);
                        if whole {
                            built.fill(ring);
                        }
                    }
                }
            }
        }

        built
    }

    /// Closed polyline: every vertex is connected to the next one, the last —
    /// to the first.
    fn line(&mut self, ring: &[Vertex]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(ring);

        let count = ring.len() as u32;
        for i in 0..count {
            self.indices.extend_from_slice(&[base + i, base + (i + 1) % count]);
        }
    }

    /// Closed ribbon: a pair of vertices per node — left edge and right — and
    /// a quad of two triangles per link.
    ///
    /// The shader spreads the vertices apart; here they lie on top of each
    /// other: by how much to spread them is known only where the frame size is
    /// known.
    fn ribbon(&mut self, ring: &[Vertex]) {
        let base = self.ribbon.len() as u32;
        let count = ring.len();
        for (index, vertex) in ring.iter().enumerate() {
            let here = (vertex.position, vertex.position_low);
            let at = |node: &Vertex| (node.position, node.position_low);
            // Offsets rather than the neighbors' positions: the high halves of
            // neighboring outline vertices coincide, and a difference taken in
            // the shader would yield zero where the neighbor is fifty metres
            // away.
            let prev = geodesy::offset(at(&ring[(index + count - 1) % count]), here);
            let next = geodesy::offset(at(&ring[(index + 1) % count]), here);
            for side in [-1.0, 1.0] {
                self.ribbon.push(RibbonVertex {
                    position: vertex.position,
                    position_low: vertex.position_low,
                    normal: vertex.normal,
                    prev,
                    next,
                    side,
                });
            }
        }

        for i in 0..count as u32 {
            let here = base + i * 2;
            let ahead = base + ((i + 1) % count as u32) * 2;
            self.ribbon_indices
                .extend_from_slice(&[here, here + 1, ahead, ahead, here + 1, ahead + 1]);
        }
    }
    /// Filling the area occupied by the scene — a fan from the ring centre out
    /// to its edge.
    ///
    /// A fan, not an ear clipping: scene outlines are quadrilaterals and
    /// imaging swaths, that is, convex, and for a convex ring a fan from its
    /// centre is the exact decomposition. On a concave ring it would spill past
    /// the edge, and in hatching this reads as a slightly wider rim rather than
    /// an error; ear clipping would cost ten times more for a case that imaging
    /// never produces.
    ///
    /// Along the radius the fan is subdivided as finely as the ring itself is
    /// densified, and for the same reason (see [`max_edge_deg`]): a triangle
    /// from the centre to the edge is a chord, and already at one degree of arc
    /// its midpoint sinks beneath the lift deeper than the outline above it is
    /// raised, where the depth test consumes it.
    ///
    /// The fan has no density ceiling, unlike the overlay warp mesh, and there
    /// is no reason to introduce one. The step is the same along the radius and
    /// along the ring ([`max_edge_deg`]), so the vertex count comes out at
    /// `360·sin θ·θ / step²`, where θ is the ring **radius**. For a Sentinel-5P
    /// swath, the widest one arriving here (2600 km across, that is a radius of
    /// about twelve degrees), this is twelve thousand vertices and half a
    /// megabyte. For a ring reaching [`FILL_REACH`] it would come out at four
    /// hundred thousand and fifteen megabytes — but such a ring would have to
    /// be nineteen thousand kilometres across, and anything wider is not filled
    /// with a fan at all, being cut off earlier.
    ///
    /// Should such a scene appear, it is handled by the same trick as the
    /// overlays: a ceiling on the fractions, with the remainder of the pass
    /// covered by the lift.
    ///
    /// Nothing is drawn if the ring has no centre: see [`FILL_REACH`].
    fn fill(&mut self, ring: &[Vertex]) {
        let direction = |vertex: &Vertex| glam::Vec3::from(vertex.normal).as_dvec3();
        let Some(centre) = ring.iter().map(direction).sum::<DVec3>().try_normalize() else {
            return;
        };
        // How far the edge lies — by the farthest vertex. The same measure
        // drives both the radial subdivision and the buildability of the fan
        // itself.
        let reach = ring.iter().map(direction).map(|point| centre.dot(point)).fold(1.0, f64::min);
        if reach < FILL_REACH {
            return;
        }
        let steps =
            (reach.clamp(-1.0, 1.0).acos().to_degrees() / max_edge_deg()).ceil().max(1.0) as u32;

        // The centre is a single vertex for the whole fan: the first ring of
        // triangles radiates from it, and quads follow between adjacent rings.
        let base = self.hatch.len() as u32;
        self.hatch.push(surface(centre));
        for step in 1..=steps {
            let part = f64::from(step) / f64::from(steps);
            for point in ring.iter().map(direction) {
                // Directions, not points: the reference origin is the Earth's
                // centre, and the renormalized direction lands on the surface
                // again. The fraction runs along the chord, not the arc: the
                // step is fine, and the irregularity of node spacing does not
                // matter to the fill. The chord cannot degenerate — the ring
                // holds no opposite points (see [`FILL_REACH`]).
                self.hatch.push(surface(centre.lerp(point, part).normalize()));
            }
        }

        // The fan rings are counted from zero, not from one: the centre stands
        // before them as a separate vertex, and a subtraction inside the count
        // would wrap `u32` below zero once.
        let count = ring.len() as u32;
        let ring_at = |level: u32| base + 1 + level * count;
        for index in 0..count {
            let (here, ahead) = (ring_at(0) + index, ring_at(0) + (index + 1) % count);
            self.hatch_indices.extend_from_slice(&[base, here, ahead]);
        }
        for level in 0..steps - 1 {
            let (inner, outer) = (ring_at(level), ring_at(level + 1));
            for index in 0..count {
                let ahead = (index + 1) % count;
                self.hatch_indices.extend_from_slice(&[
                    inner + index, outer + index, inner + ahead,
                    inner + ahead, outer + index, outer + ahead,
                ]);
            }
        }
    }
}

/// A surface point along a direction from the Earth's centre — at the same
/// height as the outlines.
fn surface(direction: DVec3) -> Vertex {
    let (lat_deg, lon_deg) = geodesy::angles(direction);
    mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: height_m() })
}

/// The outline's loops, densified to [`max_edge_deg`]. Empty — nothing to
/// enclose: two points make no closed line.
///
/// A loop is usually single: a scene outline is a quadrilateral or an imaging
/// swath. There are more of them for an outline written as a rectangle across
/// the whole Earth: it has no side edges at all, they come from the way it is
/// written (see [`seam_cuts`]).
fn ring(outline: &Outline) -> Vec<Vec<Vertex>> {
    // We lay down the closure ourselves, so a repeated last point is dropped:
    // otherwise the closing edge would degenerate into a point.
    let points = match outline.points.as_slice() {
        [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
            &outline.points[..outline.points.len() - 1]
        }
        all => all,
    };
    if points.len() < 3 {
        return Vec::new();
    }

    let at = |index: usize| {
        let point = &points[index % points.len()];
        (point.lat, point.lon)
    };
    let places: Vec<(f64, f64)> = (0..points.len()).map(at).collect();
    let sweeps: Vec<f64> =
        (0..points.len()).map(|index| geodesy::sweep(at(index), at(index + 1))).collect();
    let cuts = seam_cuts(&places, &sweeps);

    // The traversal starts right after the cut: a run started from the middle
    // would fall apart into two — the beginning of the ring would land in one
    // loop and its continuation in another.
    let start = cuts.first().map_or(0, |cut| cut + 1);
    let (mut loops, mut ring) = (Vec::new(), Vec::new());
    for step in 0..points.len() {
        let index = (start + step) % points.len();
        if cuts.contains(&index) {
            // An empty run — two cuts in a row: this comes out for an outline
            // with a repeated corner vertex, and there is no loop after it.
            if !ring.is_empty() {
                loops.push(std::mem::take(&mut ring));
            }
            continue;
        }
        edge(&mut ring, at(index), at(index + 1));
    }
    if !ring.is_empty() {
        loops.push(ring);
    }
    loops
}

/// Cut edges: those that do not lead the outline but only cross from one of its
/// edges to the other.
///
/// A belt that has gone around the Earth has no side edges: the rectangle is
/// written with four corners, and its side edges are one and the same line of
/// the globe, traced up and down. They carry no longitude, and that is exactly
/// how they are recognized.
///
/// A single zero is not enough: a zero edge is sometimes a real edge as well.
/// What is examined is the runs between the zeros, and two things are examined.
///
/// **Whether the run closed on itself.** A seam is a crossing from edge to
/// edge, which means that what it connects is one and the same place of the
/// globe even without it. If the ends do not meet — the zeros here are real
/// edges, and dropping them would pull the loop shut with a chord across
/// untouched ground.
///
/// **Whether it went around the Earth** ([`geodesy::FULL_CIRCLE_DEG`]). For a
/// global grid short of closing by ten degrees, the side edges also stand along
/// a meridian, yet the runs give ±350° — there is nothing to cut. For a belt of
/// four vertices they give ±360°. A run with zero span (like an appendage
/// "there and back") does not count at all: it does not go around a circle, but
/// it must not interfere with its neighbors either.
///
/// The trait does not depend on the vertex count, but it does depend on where
/// the vertices stand: `sweep` unwraps every edge to the nearest branch, and
/// the sum is preserved only while no edge is longer than a half circle. A belt
/// written in quarters is cut; the same belt split in halves is not.
fn seam_cuts(places: &[(f64, f64)], sweeps: &[f64]) -> Vec<usize> {
    let cuts: Vec<usize> =
        (0..sweeps.len()).filter(|index| sweeps[*index] == 0.0).collect();
    if cuts.is_empty() {
        return Vec::new();
    }

    let mut carried = false;
    for pair in 0..cuts.len() {
        let (from, to) = (cuts[pair], cuts[(pair + 1) % cuts.len()]);
        // Up to the cut itself but not including it: a run that captured its
        // own edge would close identically, and there would be nothing to ask
        // about it.
        let run: Vec<usize> = (1..sweeps.len())
            .map(|step| (from + step) % sweeps.len())
            .take_while(|index| *index != to)
            .collect();
        let Some((&first, &last)) = run.first().zip(run.last()) else {
            continue;
        };
        let (opens, closes) = (places[first], places[(last + 1) % places.len()]);
        if opens.0 != closes.0 || (opens.1 - closes.1).rem_euclid(360.0) != 0.0 {
            return Vec::new();
        }
        let span: f64 = run.iter().map(|index| sweeps[*index]).sum();
        match span.abs() >= geodesy::FULL_CIRCLE_DEG {
            true => carried = true,
            // An appendage: it walked itself there and back. It did not go
            // around the circle, but it does not vote against the cut either.
            false if span == 0.0 => continue,
            false => return Vec::new(),
        }
    }
    match carried {
        true => cuts,
        false => Vec::new(),
    }
}

/// An edge from one vertex to another, densified to [`max_edge_deg`].
///
/// Exactly how to lead the edge and how far to unwrap its longitude is decided
/// by `geodesy`: the same answer is needed by the code computing click hits,
/// and a second copy of it would silently disagree with this one (see
/// `geodesy::along`).
///
/// The end point is not placed: the next edge places it, and the last one
/// closes the outline.
fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    // No more steps than an edge needs for a full circle: an edge is never
    // longer, and the conversion to `u32` saturates instead of overflowing — a
    // number arriving over the bus would yield four billion steps with a vertex
    // at each.
    //
    // The limit is put on the steps, not the span: `f64::min` lets a NaN
    // through as its second argument, and a span clamped by it would give a
    // full circle where there is nothing to lead at all.
    let edge_deg = max_edge_deg();
    let most = (geodesy::TURN_DEG / edge_deg).ceil();
    let steps = (geodesy::edge_span(from, to) / edge_deg).ceil().max(1.0).min(most) as u32;
    let height_m = height_m();
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::edge_point(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m }));
    }
}
