use veldmap_image_tiler_wrap::pyramid;
use veldmap_image_tiler_wrap::tiles::{self, Addr, Fetch, Meta, Passes, Store};
use veldsdk::graphics::BindGroupId;

use super::camera;
use super::geodesy::{self, Geodetic, World};
use super::gpu::{OverlayVertex, Patch};
use super::cull;
use super::mesh;
use super::perf::Toll;
use super::projection;

const PATCH_SEGMENTS: u32 = 8;

const MAX_PATCH_SEGMENTS: u32 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

#[derive(Clone, Copy)]
pub struct Tie {
    pub fx: f64,
    pub fy: f64,
    pub lat: f64,
    pub lon: f64,
}

pub enum Frame {
    Projected { system: projection::Projection, affine: [f64; 6] },
    Quad([(f64, f64); 4]),
    Rough([(f64, f64); 4]),
    Grid(Grid),
}

impl Frame {
    pub fn utm(system: impl Into<projection::Projection>, x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self::Projected { system: system.into(), affine: [x1 - x0, 0.0, x0, 0.0, -(y1 - y0), y1] }
    }

    pub fn from_placement(said: &Placement) -> Result<Self, String> {
        let system = projection::Projection::from_epsg(said.epsg)
            .ok_or_else(|| format!("EPSG:{}, and we know no such system", said.epsg))?;
        let affine =
            [said.x_per_i, said.x_per_j, said.x0, said.y_per_i, said.y_per_j, said.y0];
        let frame = Self::placed(system, affine, said.width, said.height);
        if !frame.measurable() {
            return Err(format!("EPSG:{} binding has no extent", said.epsg));
        }
        Ok(frame)
    }

    fn placed(
        system: projection::Projection,
        affine: [f64; 6],
        width: u32,
        height: u32,
    ) -> Self {
        let (across, down) = (f64::from(width), f64::from(height));
        Self::Projected {
            system,
            affine: [
                affine[0] * across,
                affine[1] * down,
                affine[2],
                affine[3] * across,
                affine[4] * down,
                affine[5],
            ],
        }
    }

    pub fn quad(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Quad(points.map(|(lat, lon)| (lat, geodesy::unwind(base, lon))))
    }

    pub fn rough(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Rough(points.map(|(lat, lon)| (lat, geodesy::unwind(base, lon))))
    }

    pub fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        match self {
            Self::Projected { system, affine } => {
                let x = affine[0] * fx + affine[1] * fy + affine[2];
                let y = affine[3] * fx + affine[4] * fy + affine[5];
                system.to_geodetic(x, y)
            }
            Self::Quad([ul, ur, lr, ll]) | Self::Rough([ul, ur, lr, ll]) => {
                let top = geodesy::edge_point(*ul, *ur, fx);
                let bottom = geodesy::edge_point(*ll, *lr, fx);
                geodesy::edge_point(top, bottom, fy)
            }
            Self::Grid(grid) => grid.geodetic(fx, fy),
        }
    }

    pub fn ground_m_per_px(&self, width: u32) -> Option<f64> {
        let probed = || {
            [0.0, 0.5, 1.0]
                .iter()
                .map(|fy| {
                    let at = |fx: f64| self.geodetic(fx, *fy);
                    ground_span(at(0.0), at(0.5)) + ground_span(at(0.5), at(1.0))
                })
                .fold(0.0, f64::max)
        };
        let span = match self {
            Self::Projected { affine, .. } => affine[0].hypot(affine[3]),
            Self::Quad(_) | Self::Rough(_) => probed(),
            Self::Grid(grid) => probed().max(grid.widest_row_m),
        };
        (span.is_finite() && span > 0.0).then(|| span / f64::from(width.max(1)))
    }

    pub fn axial(&self) -> bool {
        match self {
            Self::Projected { system: projection::Projection::WebMercator, affine } => {
                affine[1] == 0.0 && affine[3] == 0.0
            }
            Self::Projected { .. } => false,
            Self::Quad([ul, ur, lr, ll]) | Self::Rough([ul, ur, lr, ll]) => {
                ul.0 == ur.0 && ll.0 == lr.0 && ul.1 == ll.1 && ur.1 == lr.1
            }
            Self::Grid(grid) => grid.axial,
        }
    }

    pub fn measurable(&self) -> bool {
        self.ground_m_per_px(1).is_some()
    }
}

pub struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    nodes: Vec<(f64, f64)>,
    axial: bool,
    widest_row_m: f64,
}

const SAME_LINE: f64 = 1e-6;

fn line(axis: &[f64], at: f64) -> Option<usize> {
    let index = axis.partition_point(|line| *line < at - SAME_LINE);
    axis.get(index).filter(|line| (**line - at).abs() <= SAME_LINE).map(|_| index)
}

impl Grid {
    pub fn new(ties: &[Tie]) -> Option<Self> {
        let axis = |values: Vec<f64>| {
            let mut values = values;
            values.sort_by(f64::total_cmp);
            values.dedup_by(|here, before| (*here - *before).abs() <= SAME_LINE);
            values
        };
        let xs = axis(ties.iter().map(|tie| tie.fx).collect());
        let ys = axis(ties.iter().map(|tie| tie.fy).collect());
        if xs.len() < 2 || ys.len() < 2 || xs.len() * ys.len() != ties.len() {
            return None;
        }

        let mut nodes = vec![None; ties.len()];
        for tie in ties {
            let col = line(&xs, tie.fx)?;
            let row = line(&ys, tie.fy)?;
            nodes[row * xs.len() + col] = Some((tie.lat, tie.lon));
        }
        let nodes: Option<Vec<(f64, f64)>> = nodes.into_iter().collect();
        let mut grid = Self { xs, ys, nodes: nodes?, axial: false, widest_row_m: 0.0 };
        grid.unwind();
        grid.axial = grid.axial();
        grid.widest_row_m = grid.widest_row();
        (grid.widest_row_m > 0.0 && grid.widest_column() > 0.0).then_some(grid)
    }

    fn unwind(&mut self) {
        let across = self.xs.len();
        let whole_circle = across == 2;
        for row in 0..self.ys.len() {
            for col in 1..across {
                let previous = self.nodes[row * across + col - 1].1;
                let node = &mut self.nodes[row * across + col];
                if whole_circle && (node.1 - previous).abs() >= geodesy::FULL_CIRCLE_DEG {
                    continue;
                }
                node.1 = geodesy::unwind(previous, node.1);
            }
            if row > 0 {
                let mut votes: Vec<i32> = (0..across)
                    .map(|col| {
                        let above = self.nodes[(row - 1) * across + col].1;
                        let here = self.nodes[row * across + col].1;
                        ((geodesy::unwind(above, here) - here) / geodesy::TURN_DEG).round() as i32
                    })
                    .collect();
                votes.sort_unstable();
                let (mut chosen, mut best, mut at) = (votes[0], 0, 0);
                while at < votes.len() {
                    let same = votes[at..].partition_point(|vote| *vote == votes[at]);
                    if same > best || (same == best && votes[at].abs() < chosen.abs()) {
                        (chosen, best) = (votes[at], same);
                    }
                    at += same;
                }
                let shift = f64::from(chosen) * geodesy::TURN_DEG;
                for col in 0..across {
                    self.nodes[row * across + col].1 += shift;
                }
            }
        }
    }

    fn axial(&self) -> bool {
        let across = self.xs.len();
        let rows = self.nodes.chunks_exact(across);
        rows.clone().all(|row| row.iter().all(|node| node.0 == row[0].0))
            && (0..across).all(|col| {
                rows.clone().all(|row| row[col].1 == self.nodes[col].1)
            })
    }

    fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        let (col, tx) = cell(&self.xs, fx);
        let (row, ty) = cell(&self.ys, fy);
        let node = |row: usize, col: usize| self.nodes[row * self.xs.len() + col];
        let top = lerp(node(row, col), node(row, col + 1), tx);
        let bottom = lerp(node(row + 1, col), node(row + 1, col + 1), tx);
        lerp(top, bottom, ty)
    }

    pub fn is_dense(&self) -> bool {
        self.xs.len() * self.ys.len() > 4
    }

    fn widest_row(&self) -> f64 {
        let across = self.xs.len();
        let width = (self.xs[across - 1] - self.xs[0]).max(f64::EPSILON);
        self.nodes
            .chunks_exact(across)
            .map(|row| {
                row.windows(2).map(|pair| ground_span(pair[0], pair[1])).sum::<f64>() / width
            })
            .fold(0.0, f64::max)
    }

    fn widest_column(&self) -> f64 {
        let across = self.xs.len();
        let height = (self.ys[self.ys.len() - 1] - self.ys[0]).max(f64::EPSILON);
        (0..across)
            .map(|col| {
                (1..self.ys.len())
                    .map(|row| {
                        ground_span(self.nodes[(row - 1) * across + col], self.nodes[row * across + col])
                    })
                    .sum::<f64>()
                    / height
            })
            .fold(0.0, f64::max)
    }
}

fn cell(axis: &[f64], at: f64) -> (usize, f64) {
    let last = axis.len() - 2;
    let i = axis.partition_point(|edge| *edge <= at).saturating_sub(1).min(last);
    (i, (at - axis[i]) / (axis[i + 1] - axis[i]))
}

fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn ground_span(from: (f64, f64), to: (f64, f64)) -> f64 {
    arc_deg(from, to).to_radians() * geodesy::SEMI_MAJOR_M
}

fn arc_deg(from: (f64, f64), to: (f64, f64)) -> f64 {
    let along = (to.1 - from.1) * ((from.0 + to.0) * 0.5).to_radians().cos();
    (to.0 - from.0).hypot(along)
}

fn wire_variable(variable: &tiles::Variable) -> crate::proto::globe::Variable {
    crate::proto::globe::Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

pub struct Raster {
    pub role: Role,
    pub resource: veldsdk::OwnedResource,
    pub geolocation: Option<veldsdk::OwnedResource>,
    pub ordinal: u32,
    pub variable: String,
    pub variables: Vec<tiles::Variable>,
    pub spares: Vec<(u32, veldsdk::OwnedResource, Option<veldsdk::OwnedResource>)>,
    pub meta: Option<Meta>,
    pub trouble: Option<String>,
    pub describe: veldsdk::Latest,
    pub fetch: Fetch,
    pub bounds: Vec<Vec<cull::Ball>>,
    pub pass: (u64, u64),
    mesh: std::collections::HashMap<tiles::Addr, Nodes>,
    meshed: u64,
}

impl Raster {
    pub fn new(role: Role, resource: veldsdk::OwnedResource) -> Self {
        Self {
            role,
            resource,
            geolocation: None,
            ordinal: 0,
            variable: String::new(),
            variables: Vec::new(),
            spares: Vec::new(),
            meta: None,
            bounds: Vec::new(),
            trouble: None,
            describe: veldsdk::Latest::default(),
            fetch: Fetch::default(),
            pass: (0, 0),
            mesh: std::collections::HashMap::new(),
            meshed: 0,
        }
    }

    pub fn next_spare(&mut self) -> Option<(veldsdk::ResourceHandle, Option<veldsdk::ResourceHandle>)> {
        if self.spares.is_empty() {
            return None;
        }
        let (ordinal, resource, geolocation) = self.spares.remove(0);
        let spares = std::mem::take(&mut self.spares);
        let mut fresh = Raster::new(self.role, resource);
        fresh.ordinal = ordinal;
        fresh.geolocation = geolocation;
        fresh.spares = spares;
        *self = fresh;
        Some((self.resource.as_ref().clone(), self.geolocation.as_ref().map(|owned| owned.as_ref().clone())))
    }

    pub fn refused(&mut self, error: String) -> Option<(veldsdk::ResourceHandle, Option<veldsdk::ResourceHandle>)> {
        if !self.variable.is_empty() {
            self.trouble = Some(error);
            return None;
        }
        if let Some(next) = self.next_spare() {
            return Some(next);
        }
        self.trouble = Some(match self.role {
            Role::Detailed => format!("detailed raster did not open: {}", error),
            Role::Preview => format!("preview did not open: {}", error),
        });
        None
    }

    pub fn describe_as(&mut self, meta: Meta) {
        if !meta.variables.is_empty() {
            self.variables = meta.variables.clone();
        }
        self.meta = Some(meta);
        self.mesh.clear();
    }

    fn keep_mesh(&mut self, relaid: u64, cells: &[tiles::Addr]) {
        if self.meshed != relaid {
            self.mesh.clear();
            self.meshed = relaid;
        }
        self.mesh.retain(|addr, _| cells.binary_search(addr).is_ok());
    }

    pub fn read(&self) -> f64 {
        match self.pass {
            (_, 0) => 0.0,
            (read, total) => (read as f64 / total as f64).clamp(0.0, 1.0),
        }
    }
}

pub struct Overlay {
    pub key: String,
    pub label: String,
    pub frame: Frame,
    pub relaid: u64,
    pub binding: Binding,
    pub binding_trouble: Option<String>,
    pub rasters: Vec<Raster>,
    pub sources: Vec<u64>,
    pub variables: Vec<String>,
    pub opacity: f32,
    pub hidden: bool,
    pub error: String,
    pub progress: Progress,
}

pub struct Placement {
    pub epsg: u32,
    pub x_per_i: f64,
    pub x_per_j: f64,
    pub x0: f64,
    pub y_per_i: f64,
    pub y_per_j: f64,
    pub y0: f64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    Catalogue,
    Named,
    Projected,
    Lattice,
}

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Progress {
    pub ready: u32,
    pub total: u32,
    pub share: f32,
    pub step: u32,
    pub steps: u32,
    pub within: f32,
    pub pass: (u64, u64),
}

pub fn progress_of(raster: &Raster, want: &tiles::Want) -> Progress {
    let (ready, total) = raster.fetch.ordered();
    let pointwise =
        raster.meta.as_ref().is_some_and(|meta| meta.pointwise(want.level));
    let inside = tiles::inside((ready, total), raster.pass, pointwise);
    Progress {
        ready,
        total,
        share: ((f64::from(want.climbed) + inside.share) / f64::from(want.steps.max(1)))
            .clamp(0.0, 1.0) as f32,
        step: want.climbed,
        steps: want.steps,
        within: inside.share as f32,
        pass: inside.read,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub role: Role,
    pub fingerprint: String,
    pub level: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    key: String,
    opacity: f32,
    relaid: u64,
    choice: Choice,
    cells: Vec<Drawn>,
}

pub struct Wanted {
    pub choice: Choice,
    pub want: tiles::Want,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Look {
    pub view_proj: camera::Mat4,
    pub eye: World,
    pub mpp: f64,
}

impl Overlay {
    pub fn trouble(&self) -> String {
        let said: Vec<String> = self
            .rasters
            .iter()
            .filter_map(|raster| raster.trouble.clone())
            .chain(self.binding_trouble.clone())
            .collect();
        said.join("; ")
    }

    pub fn report<K: PartialEq>(
        &self,
        mine: Option<&Wanted>,
        passes: &Passes<K>,
    ) -> crate::proto::globe::OverlayProgress {
        let live = mine.is_some();
        let working = self.working(passes, mine, live);
        let drawn = mine.map(|wanted| wanted.choice.role);
        let settled = !working && drawn == self.decider().map(|it| it.role);
        let (detailed_trouble, trouble) = self.said_split(settled);
        crate::proto::globe::OverlayProgress {
            key: self.key.clone(),
            ready: self.progress.ready,
            total: self.progress.total,
            working,
            share: self.progress.share,
            error: self.error.clone(),
            trouble,
            step: self.progress.step,
            steps: self.progress.steps,
            blank: self.blank(),
            pass_read: self.progress.pass.0,
            pass_total: self.progress.pass.1,
            detailed: self.raster(Role::Detailed).map(|raster| raster.ordinal),
            detailed_trouble,
            detailed_variable: self
                .raster(Role::Detailed)
                .and_then(|raster| raster.meta.as_ref())
                .and_then(|meta| meta.variable.as_ref())
                .map(wire_variable),
            detailed_variables: self
                .raster(Role::Detailed)
                .map(|raster| raster.variables.iter().map(wire_variable).collect())
                .unwrap_or_default(),
        }
    }

    pub fn said(&self, settled: bool) -> String {
        let cap = settled.then(|| self.detail_cap()).flatten();
        let said: Vec<String> = self
            .rasters
            .iter()
            .filter_map(|raster| raster.trouble.clone())
            .chain(cap)
            .chain(self.binding_trouble.clone())
            .collect();
        said.join("; ")
    }

    pub fn said_split(&self, settled: bool) -> (String, String) {
        let cap = settled.then(|| self.detail_cap()).flatten();
        let cap_about_detailed = self.decider().is_some_and(|raster| raster.role == Role::Detailed);
        let (mut about_detailed, mut rest) = (Vec::new(), Vec::new());
        for raster in &self.rasters {
            if let Some(trouble) = &raster.trouble {
                match raster.role {
                    Role::Detailed => about_detailed.push(trouble.clone()),
                    Role::Preview => rest.push(trouble.clone()),
                }
            }
        }
        if let Some(cap) = cap {
            match cap_about_detailed {
                true => about_detailed.push(cap),
                false => rest.push(cap),
            }
        }
        rest.extend(self.binding_trouble.clone());
        (about_detailed.join("; "), rest.join("; "))
    }

    fn decider(&self) -> Option<&Raster> {
        self.budgeted()
            .filter(|raster| raster.meta.is_some())
            .max_by_key(|raster| raster.meta.as_ref().expect("selected by presence").reachable().0)
    }

    fn detail_cap(&self) -> Option<String> {
        self.decider()?.meta.as_ref()?.capped()
    }

    pub fn doomed(&self) -> bool {
        !self.rasters.iter().any(|raster| raster.meta.is_some())
            && self.rasters.iter().all(|raster| raster.variable.is_empty())
    }

    pub fn revariabled(&self, wanted: &[(u64, String)]) -> Vec<Role> {
        self.rasters
            .iter()
            .filter(|raster| {
                let asked = wanted
                    .iter()
                    .find(|(id, _)| *id == raster.resource.as_ref().id)
                    .map(|(_, variable)| variable.as_str())
                    .unwrap_or_default();
                asked != raster.variable
            })
            .map(|raster| raster.role)
            .collect()
    }

    pub fn raster_mut(&mut self, role: Role) -> Option<&mut Raster> {
        self.rasters.iter_mut().find(|raster| raster.role == role)
    }

    pub fn raster(&self, role: Role) -> Option<&Raster> {
        self.rasters.iter().find(|raster| raster.role == role)
    }

    pub fn working<K: PartialEq>(
        &self,
        passes: &Passes<K>,
        wanted: Option<&Wanted>,
        live: bool,
    ) -> bool {
        self.rasters.iter().any(|raster| {
            if raster.describe.is_pending() {
                return true;
            }
            if !live {
                return false;
            }
            let Some(meta) = raster.meta.as_ref() else { return false };
            let mine = wanted
                .filter(|wanted| wanted.choice.role == raster.role)
                .map(|wanted| &wanted.want);
            tiles::working(&raster.fetch, passes, &meta.fingerprint, mine)
        })
    }

    pub fn blank(&self) -> bool {
        self.binding_pending() || !self.rasters.iter().any(|raster| raster.meta.is_some())
    }

    fn binding_pending(&self) -> bool {
        match self.frame {
            Frame::Rough(_) => true,
            Frame::Quad(_) => self.rasters.iter().any(|raster| raster.describe.is_pending()),
            Frame::Projected { .. } | Frame::Grid(_) => false,
        }
    }

    pub fn wanted(&self, look: &Look, cap_tiles: u64, store: &Store, toll: &Toll) -> Vec<Wanted> {
        if self.binding_pending() {
            return Vec::new();
        }
        let mut wanted = Vec::new();
        let described = |role: Role| {
            let raster = self.raster(role)?;
            Some((raster, raster.meta.as_ref()?))
        };

        if !self.frame.measurable() {
            return Vec::new();
        }
        let base = described(Role::Preview);
        if let Some((raster, meta)) = base {
            wanted.push(self.at_level(raster, meta, look, cap_tiles, store, toll));
            if self.frame.ground_m_per_px(meta.width).is_some_and(|mpp| look.mpp >= mpp) {
                return wanted;
            }
        }
        if let Some((raster, meta)) = described(Role::Detailed)
            && !self.detail_eclipsed()
        {
            wanted.push(self.at_level(raster, meta, look, cap_tiles, store, toll));
        }
        wanted
    }

    pub fn detail_eclipsed(&self) -> bool {
        let width = |role| {
            let meta = self.raster(role)?.meta.as_ref()?;
            Some(meta.reachable().0)
        };
        match (width(Role::Preview), width(Role::Detailed)) {
            (Some(base), Some(detail)) => detail <= base,
            _ => false,
        }
    }

    pub fn budgeted(&self) -> impl Iterator<Item = &Raster> {
        let eclipsed = self.detail_eclipsed();
        self.rasters.iter().filter(move |raster| !(eclipsed && raster.role == Role::Detailed))
    }

    pub fn built(&self, wanted: &Wanted, drawn: &[Drawn]) -> Built {
        Built {
            key: self.key.clone(),
            opacity: self.opacity,
            relaid: self.relaid,
            choice: wanted.choice.clone(),
            cells: drawn.to_vec(),
        }
    }

    pub fn relay(&mut self, frame: Frame, binding: Binding) {
        self.frame = frame;
        self.binding = binding;
        self.relaid += 1;
        self.binding_trouble = None;
    }

    pub fn complain(&mut self, said: String) {
        if self.binding >= Binding::Projected {
            return;
        }
        match &mut self.binding_trouble {
            Some(had) if had.contains(&said) => {}
            Some(had) => {
                had.push_str("; ");
                had.push_str(&said);
            }
            none => *none = Some(said),
        }
    }

    fn at_level(
        &self,
        raster: &Raster,
        meta: &Meta,
        look: &Look,
        cap_tiles: u64,
        store: &Store,
        toll: &Toll,
    ) -> Wanted {
        let want = tiles::want(
            self.sharpest(meta, look),
            meta,
            cap_tiles,
            store,
            &raster.fetch,
            |level| self.visible(meta, &raster.bounds, level, look, toll),
        );
        Wanted {
            choice: Choice { role: raster.role, fingerprint: meta.fingerprint.clone(), level: want.level },
            want,
        }
    }

    fn sharpest(&self, meta: &Meta, look: &Look) -> u32 {
        let Some(mpp_raster) = self.frame.ground_m_per_px(meta.width) else { return 0 };
        match look.mpp <= mpp_raster {
            true => 0,
            false => (look.mpp / mpp_raster).log2().floor() as u32,
        }
    }

    pub fn bounds(&self, meta: &Meta) -> Vec<Vec<cull::Ball>> {
        (0..meta.levels())
            .map(|level| {
                let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
                let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
                (0..grid_h)
                    .flat_map(|y| (0..grid_w).map(move |x| (x, y)))
                    .map(|(x, y)| self.ball(meta, level, x, y))
                    .collect()
            })
            .collect()
    }

    fn ball(&self, meta: &Meta, level: u32, x: u32, y: u32) -> cull::Ball {
        let (points, seen) = self.probes(meta, level, x, y);
        let span = span_of(&seen);
        let lifted = mesh::lift_m(span / f64::from(segments_for(span)) * std::f64::consts::SQRT_2)
            - mesh::SURFACE_LIFT_M;
        let reach = covering_radius_deg(&seen).to_radians() * mesh::drawn_radius();
        cull::Ball::over(&points).widened(reach as f32 + geodesy::metres(lifted))
    }

    fn probes(&self, meta: &Meta, level: u32, x: u32, y: u32) -> ([World; 9], [(f64, f64); 9]) {
        let cell = pyramid::cell_image_rect(x, y, level, meta.width, meta.height);
        let along = |from: f64, to: f64, at: usize| from + (to - from) * (at as f64) / 2.0;
        let mut points = [[0.0f32; 3]; 9];
        let mut seen = [(0.0f64, 0.0f64); 9];
        for at in 0..9 {
            let (px, py) = (along(cell[0], cell[2], at % 3), along(cell[1], cell[3], at / 3));
            let (lat, lon) = self
                .frame
                .geodetic(px / f64::from(meta.width), py / f64::from(meta.height));
            seen[at] = (lat, lon);
            points[at] = geodesy::position(Geodetic {
                lat_deg: lat,
                lon_deg: lon,
                height_m: mesh::SURFACE_LIFT_M,
            });
        }
        (points, seen)
    }

    fn visible(
        &self,
        meta: &Meta,
        bounds: &[Vec<cull::Ball>],
        level: u32,
        look: &Look,
        toll: &Toll,
    ) -> Vec<Addr> {
        let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
        let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
        let frame = cull::Frame::new(&look.view_proj, look.eye);
        let balls = bounds.get(level as usize);
        let mut cells = Vec::new();
        for y in 0..grid_h {
            for x in 0..grid_w {
                if let Some(ball) = balls.and_then(|balls| balls.get((y * grid_w + x) as usize))
                    && !frame.admits(ball)
                {
                    continue;
                }
                if self.on_screen(meta, level, x, y, look) {
                    cells.push((level, x, y));
                }
            }
        }
        toll.level(u64::from(grid_w) * u64::from(grid_h), cells.len() as u64);
        cells
    }

    fn on_screen(&self, meta: &Meta, level: u32, x: u32, y: u32, look: &Look) -> bool {
        let (world, seen) = self.probes(meta, level, x, y);

        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        let mut faces = false;
        for point in world {
            faces |= faces_eye(point, look.eye);

            let clip = camera::project(&look.view_proj, point, look.eye);
            if clip[3] <= 0.0 {
                return true;
            }
            for axis in 0..2 {
                let ndc = clip[axis] / clip[3];
                low[axis] = low[axis].min(ndc);
                high[axis] = high[axis].max(ndc);
            }
        }

        let faces = faces || faces_cap(&world, look.eye, covering_radius_deg(&seen));
        faces && low[0] <= 1.0 && high[0] >= -1.0 && low[1] <= 1.0 && high[1] >= -1.0
    }
}

fn faces_eye(point: World, eye: World) -> bool {
    let to_eye = [eye[0] - point[0], eye[1] - point[1], eye[2] - point[2]];
    point[0] * to_eye[0] + point[1] * to_eye[1] + point[2] * to_eye[2] > 0.0
}

fn covering_radius_deg(seen: &[(f64, f64); 9]) -> f64 {
    let (mut step_x, mut step_y) = (0.0f64, 0.0f64);
    for at in 0..3 {
        for step in 0..2 {
            let row = at * 3 + step;
            step_x = step_x.max(arc_deg(seen[row], seen[row + 1]));
            let column = step * 3 + at;
            step_y = step_y.max(arc_deg(seen[column], seen[column + 3]));
        }
    }
    0.5 * step_x.hypot(step_y)
}

fn faces_cap(points: &[World; 9], eye: World, spread_deg: f64) -> bool {
    let dot = |a: &World, b: &World| {
        f64::from(a[0]) * f64::from(b[0])
            + f64::from(a[1]) * f64::from(b[1])
            + f64::from(a[2]) * f64::from(b[2])
    };
    let mut lengths = [0.0f64; 9];
    for (length, point) in lengths.iter_mut().zip(points) {
        *length = dot(point, point);
    }
    let lowest = lengths.iter().copied().fold(f64::MAX, f64::min);
    let radius = lowest.sqrt();
    let beyond = (dot(&eye, &eye) - lowest).max(0.0).sqrt();

    let (sin_spread, cos_spread) = spread_deg.to_radians().sin_cos();
    if beyond * cos_spread + radius * sin_spread < 0.0 {
        return true;
    }
    let reach = radius * cos_spread - beyond * sin_spread;
    points
        .iter()
        .zip(&lengths)
        .any(|(point, length)| dot(point, &eye) > length.sqrt() * reach)
}

pub struct Piece {
    cell: tiles::Addr,
    carrier: tiles::Addr,
    bind: BindGroupId,
    rect: [f64; 4],
    uv: [f32; 4],
    coming: bool,
}

pub type Drawn = (tiles::Addr, tiles::Addr, u64, bool);

impl Piece {
    pub fn drawn(&self) -> Drawn {
        (self.cell, self.carrier, self.bind.id(), self.coming)
    }

    pub fn coming(&self) -> bool {
        self.coming
    }
}

pub fn pieces(overlay: &Overlay, wanted: &Wanted, store: &mut Store) -> Vec<Piece> {
    let Some(raster) = overlay.raster(wanted.choice.role) else { return Vec::new() };
    let Some(meta) = raster.meta.as_ref() else { return Vec::new() };
    let level = wanted.choice.level;

    let mut found = Vec::with_capacity(wanted.want.cells.len());
    for &cell in &wanted.want.cells {
        let Some((carrier, stored)) = store.carrier(&meta.fingerprint, cell, meta.levels()) else {
            continue;
        };
        let (bind, tex) = (stored.bind.clone(), (stored.width, stored.height));
        let rect = pyramid::cell_image_rect(cell.1, cell.2, level, meta.width, meta.height);
        let coming = carrier != cell && raster.fetch.coming(cell);
        found.push(Piece {
            cell,
            carrier,
            bind,
            uv: pyramid::cell_uv(rect, carrier, tex.0, tex.1),
            rect,
            coming,
        });
    }
    found
}

pub fn patches(
    overlay: &mut Overlay,
    wanted: &Wanted,
    pieces: &[Piece],
    vertices: &mut Vec<OverlayVertex>,
    draws: &mut Vec<Patch>,
) {
    let Overlay { frame, opacity, relaid, rasters, .. } = overlay;
    let Some(raster) = rasters.iter_mut().find(|raster| raster.role == wanted.choice.role) else {
        return;
    };
    let mut drawn: Vec<tiles::Addr> = pieces.iter().map(|piece| piece.cell).collect();
    drawn.sort_unstable();
    raster.keep_mesh(*relaid, &drawn);

    let Raster { meta, mesh, .. } = raster;
    let Some(meta) = meta.as_ref() else { return };
    for piece in pieces {
        let nodes = mesh.entry(piece.cell).or_insert_with(|| nodes_of(frame, meta, piece.rect));
        let base = vertices.len() as u32;
        spread(nodes, piece.uv, *opacity, ripple_of(piece.cell, piece.coming), vertices);
        draws.push(Patch { bind: piece.bind.clone(), segments: nodes.segments, base });
    }
}

const RIPPLE_OFFSETS: u32 = 16;

fn ripple_of(cell: tiles::Addr, coming: bool) -> f32 {
    if !coming {
        return 0.0;
    }
    let (_, x, y) = cell;
    let slot = (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(11))) % RIPPLE_OFFSETS;
    (slot as f32 + 0.5) / RIPPLE_OFFSETS as f32
}

struct Nodes {
    segments: u32,
    points: Vec<(geodesy::World, geodesy::World)>,
}

fn nodes_of(frame: &Frame, meta: &Meta, cell: [f64; 4]) -> Nodes {
    let span = span_deg(frame, meta, cell);
    let segments = segments_for(span);
    let height_m = mesh::lift_m(span / f64::from(segments) * std::f64::consts::SQRT_2);

    let side = segments + 1;
    let mut points = Vec::with_capacity((side * side) as usize);
    let at = |tx: f64, ty: f64| {
        let fx = (cell[0] + (cell[2] - cell[0]) * tx) / f64::from(meta.width);
        let fy = (cell[1] + (cell[3] - cell[1]) * ty) / f64::from(meta.height);
        frame.geodetic(fx, fy)
    };
    let share = |step: u32| f64::from(step) / f64::from(segments);

    if frame.axial() {
        let lons: Vec<(f64, f64)> =
            (0..=segments).map(|col| at(share(col), 0.0).1.to_radians().sin_cos()).collect();
        for row in 0..=segments {
            let parallel = geodesy::parallel(at(0.0, share(row)).0, height_m);
            points.extend(lons.iter().map(|&(sin, cos)| geodesy::parts(parallel.at(sin, cos))));
        }
        return Nodes { segments, points };
    }

    for row in 0..=segments {
        for col in 0..=segments {
            let (lat, lon) = at(share(col), share(row));
            points.push(geodesy::parts(geodesy::world(Geodetic {
                lat_deg: lat,
                lon_deg: lon,
                height_m,
            })));
        }
    }
    Nodes { segments, points }
}

fn spread(
    nodes: &Nodes,
    uv: [f32; 4],
    alpha: f32,
    ripple: f32,
    vertices: &mut Vec<OverlayVertex>,
) {
    let (segments, side) = (nodes.segments, nodes.segments + 1);
    debug_assert_eq!(nodes.points.len(), (side * side) as usize, "node count does not match the density");
    let across = segments as f32;
    let us: Vec<f32> = (0..side).map(|col| uv[0] + (uv[2] - uv[0]) * (col as f32 / across)).collect();
    vertices.reserve(nodes.points.len());
    for (row, line) in nodes.points.chunks(side as usize).enumerate() {
        let v = uv[1] + (uv[3] - uv[1]) * (row as f32 / across);
        for (&u, &(position, position_low)) in us.iter().zip(line) {
            vertices.push(OverlayVertex { position, position_low, uv: [u, v], alpha, ripple });
        }
    }
}

fn segments_for(span_deg: f64) -> u32 {
    ((span_deg / mesh::max_quad_side_deg()).ceil() as u32)
        .clamp(PATCH_SEGMENTS, MAX_PATCH_SEGMENTS)
}

fn span_of(seen: &[(f64, f64); 9]) -> f64 {
    let mut span: f64 = 0.0;
    for row in 0..3 {
        span = span.max(arc_deg(seen[row * 3], seen[row * 3 + 2]));
    }
    for col in 0..3 {
        span = span.max(arc_deg(seen[col], seen[6 + col]));
    }
    span
}

fn span_deg(frame: &Frame, meta: &Meta, cell: [f64; 4]) -> f64 {
    let at = |x: f64, y: f64| frame.geodetic(x / f64::from(meta.width), y / f64::from(meta.height));
    let xs = [cell[0], (cell[0] + cell[2]) * 0.5, cell[2]];
    let ys = [cell[1], (cell[1] + cell[3]) * 0.5, cell[3]];

    let mut span: f64 = 0.0;
    for &y in &ys {
        span = span.max(arc_deg(at(xs[0], y), at(xs[2], y)));
    }
    for &x in &xs {
        span = span.max(arc_deg(at(x, ys[0]), at(x, ys[2])));
    }
    span
}
