use glam::DVec3;

use crate::module::geodesy::{self, Geodetic};
use crate::module::gpu::RibbonVertex;
use crate::module::mesh::{self, Vertex};
use crate::proto::globe::{Outline, OutlineStyle};

fn height_m() -> f64 {
    mesh::lift_m(max_edge_deg() * 2.0)
}

fn max_edge_deg() -> f64 {
    mesh::max_chord_deg() / 2.0
}

const FILL_REACH: f64 = 0.087; // cos 85°

pub struct Outlines {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub ribbon: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
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
            let loops = ring(outline);
            let drawn: Vec<&Vec<Vertex>> = loops.iter().filter(|run| run.len() >= 3).collect();
            let whole = drawn.len() == 1;
            for ring in drawn {
                match outline.style() {
                    OutlineStyle::OutlineSelected => built.ribbon(ring),
                    OutlineStyle::OutlinePlain => built.line(ring),
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

    fn line(&mut self, ring: &[Vertex]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(ring);

        let count = ring.len() as u32;
        for i in 0..count {
            self.indices.extend_from_slice(&[base + i, base + (i + 1) % count]);
        }
    }

    fn ribbon(&mut self, ring: &[Vertex]) {
        let base = self.ribbon.len() as u32;
        let count = ring.len();
        for (index, vertex) in ring.iter().enumerate() {
            let here = (vertex.position, vertex.position_low);
            let at = |node: &Vertex| (node.position, node.position_low);
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
    fn fill(&mut self, ring: &[Vertex]) {
        let direction = |vertex: &Vertex| glam::Vec3::from(vertex.normal).as_dvec3();
        let Some(centre) = ring.iter().map(direction).sum::<DVec3>().try_normalize() else {
            return;
        };
        let reach = ring.iter().map(direction).map(|point| centre.dot(point)).fold(1.0, f64::min);
        if reach < FILL_REACH {
            return;
        }
        let steps =
            (reach.clamp(-1.0, 1.0).acos().to_degrees() / max_edge_deg()).ceil().max(1.0) as u32;

        let base = self.hatch.len() as u32;
        self.hatch.push(surface(centre));
        for step in 1..=steps {
            let part = f64::from(step) / f64::from(steps);
            for point in ring.iter().map(direction) {
                self.hatch.push(surface(centre.lerp(point, part).normalize()));
            }
        }

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

fn surface(direction: DVec3) -> Vertex {
    let (lat_deg, lon_deg) = geodesy::angles(direction);
    mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: height_m() })
}

fn ring(outline: &Outline) -> Vec<Vec<Vertex>> {
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

    let start = cuts.first().map_or(0, |cut| cut + 1);
    let (mut loops, mut ring) = (Vec::new(), Vec::new());
    for step in 0..points.len() {
        let index = (start + step) % points.len();
        if cuts.contains(&index) {
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

fn seam_cuts(places: &[(f64, f64)], sweeps: &[f64]) -> Vec<usize> {
    let cuts: Vec<usize> =
        (0..sweeps.len()).filter(|index| sweeps[*index] == 0.0).collect();
    if cuts.is_empty() {
        return Vec::new();
    }

    let mut carried = false;
    for pair in 0..cuts.len() {
        let (from, to) = (cuts[pair], cuts[(pair + 1) % cuts.len()]);
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
            false if span == 0.0 => continue,
            false => return Vec::new(),
        }
    }
    match carried {
        true => cuts,
        false => Vec::new(),
    }
}

fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    let edge_deg = max_edge_deg();
    let most = (geodesy::TURN_DEG / edge_deg).ceil();
    let steps = (geodesy::edge_span(from, to) / edge_deg).ceil().max(1.0).min(most) as u32;
    let height_m = height_m();
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::edge_point(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m }));
    }
}
