//! Overlays: imagery draped over the surface as pyramid tiles.
//!
//! An overlay is a binding plus up to two rasters by role: the preview gives a
//! picture at once from one small file, while the detailed one goes through the
//! pipeline on zoom-in. Tiles are fetched with the same dance as the preview
//! canvas: describe -> cache query -> produce misses, and drawn as warp-mesh
//! patches: the nodes of each cell are mapped by the binding to degrees and
//! further by geodesy to the world, and the GPU interpolates between them. The
//! raster itself is never resampled -- the mesh takes over the projection
//! distortion.
//!
//! Bindings are the variants of [`Binding`], and their precedence is the order
//! of declaration, from weakest to strongest: the footprint quad from the
//! catalogue, the zone frame from whoever knows the product layout, the
//! projection recorded in the raster itself, and the grid of its tie points.
//! The weakest answers a different question (which piece of Earth was shot,
//! not which pixel goes where) and therefore stays a guess: until the raster
//! descriptions finish, the overlay neither draws on it nor requests tiles
//! (see [`Overlay::binding_pending`]).
//!
//! A cell is drawn by exactly one carrier -- the exact tile or a piece of the
//! nearest ancestor (parent-fallback, the same pyramid.rs arithmetic as on the
//! canvas) -- so there are no overlaps inside an overlay.

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

/// How many segments a warp-mesh cell side is split into -- never fewer.
/// Eight suffice for a whole granule-sized cell (110 km): a link comes out at
/// 14 km, that is three times shorter than the limit.
const PATCH_SEGMENTS: u32 = 8;

/// Density ceiling. A cell of the top level of a global raster spans all 360°,
/// and honest division down to `mesh::max_quad_side_deg()` would cost nine
/// hundred links per side, close to a million quads for that one cell. It is
/// cheaper to make up the rest with height -- see [`nodes_of`].
///
/// Not only it hits the ceiling: sixty-four links cover a span up to
/// twenty-four degrees, and anything wider rises above the common lift. This
/// is visible only from a distance at which such a cell fits entirely in the
/// frame -- and there the rise hides under a pixel.
const MAX_PATCH_SEGMENTS: u32 = 64;

/// Raster role -- the same two as in types.proto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

/// A node of the binding grid: a raster fraction (0..1) and a place on Earth.
/// Pixels are converted to fractions when the grid is assembled -- the overlay
/// has two rasters of different sizes, and they lie the same way.
#[derive(Clone, Copy)]
pub struct Tie {
    pub fx: f64,
    pub fy: f64,
    pub lat: f64,
    pub lon: f64,
}

/// Binding of a raster to the Earth.
pub enum Frame {
    /// Exact projection binding: raster fraction -> zone metres -> degrees, and
    /// the conversion is computed at every warp-mesh node, not between corners.
    /// Hence it has no interpolation error at all: a line of constant northern
    /// offset is not a line of constant latitude, and anyone walking between
    /// corners cuts that curve off. For a quad whose edges are led by an arc --
    /// that is, for any slanted acquisition -- the cut is small: two dozen
    /// metres on a 239 km Landsat scene. Straight in degrees it would be a
    /// kilometre at mid-latitudes, but nobody walks that way here (see
    /// [`Frame::geodetic`]).
    ///
    /// Affine, not by edges: the same field also lays a rotated raster
    /// (ModelTransformation), and the axis-aligned rectangle is its special
    /// case with zeros off the diagonal (see [`Frame::utm`]). A separate variant
    /// for it would be a second name for the same thing.
    ///
    /// `x = a[0]·fx + a[1]·fy + a[2]`, `y = a[3]·fx + a[4]·fy + a[5]`, where
    /// `fx`, `fy` are raster fractions.
    Projected { system: projection::Projection, affine: [f64; 6] },
    /// The footprint quad in raster order: UL, UR, LR, LL. Longitudes are
    /// unwound into a continuous branch in advance (see [`Frame::quad`]).
    ///
    /// A guess, not a binding: the catalogue says which piece of Earth was
    /// shot, but not which pixel goes where -- its vertex order is its own. It
    /// suffices until a grid arrives from the raster itself (see
    /// [`Frame::Grid`]).
    Quad([(f64, f64); 4]),
    /// Bounds of a complex outline -- a place, not a binding. An imaging strip
    /// is traced by dozens of vertices, and stretching the raster onto its
    /// bounds means showing it in the wrong place and the wrong way. So nothing
    /// is drawn or requested by such a frame (see
    /// [`Overlay::binding_pending`]): it holds the place until the raster itself
    /// brings the binding. If it never does, there is nothing to overlay, and
    /// the layer ends with an error.
    ///
    /// The coordinates in it are still real: the middle of the bounds is the
    /// middle of the shot piece of Earth, and aiming at the layer works through
    /// it.
    Rough([(f64, f64); 4]),
    /// The grid of tie points from the raster itself -- the only binding nobody
    /// invented.
    Grid(Grid),
}

impl Frame {
    /// A projection frame by its edges in zone metres; `y1` is the northern
    /// edge.
    ///
    /// This is how the MGRS layout names it -- a 100 m grid square from its
    /// corner -- and it is equally convenient to name it this way in checks.
    /// Inside it is the same affine transform as a binding from the file: one
    /// road to degrees, not two similar ones.
    pub fn utm(system: impl Into<projection::Projection>, x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        // The raster fraction goes down while the northern coordinate goes up:
        // hence the minus.
        Self::Projected { system: system.into(), affine: [x1 - x0, 0.0, x0, 0.0, -(y1 - y0), y1] }
    }

    /// A frame from the binding read in the raster itself: that one is named in
    /// pixels, while the frame lives in raster fractions.
    ///
    /// In fractions because the overlay has two rasters of different sizes --
    /// preview and detailed -- and both lie the same way: the image has one
    /// binding, and converted to fractions it serves both.
    /// A frame from the binding read in the raster itself.
    ///
    /// `Err` -- the system cannot be converted or the transform has no extent.
    /// The reason is in words because it must be spoken aloud: with "there is no
    /// binding" one cannot tell inability from the file's silence, and there
    /// would be nothing to examine.
    pub fn from_placement(said: &Placement) -> Result<Self, String> {
        let system = projection::Projection::from_epsg(said.epsg)
            .ok_or_else(|| format!("EPSG:{}, and we know no such system", said.epsg))?;
        let affine =
            [said.x_per_i, said.x_per_j, said.x0, said.y_per_i, said.y_per_j, said.y0];
        let frame = Self::placed(system, affine, said.width, said.height);
        // A frame without extent breaks level selection fatally (see
        // [`Frame::ground_m_per_px`]), while the catalogue outline is still
        // whole. This also catches what the tiler will not emit: `NaN` among
        // the six.
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
        // `fx` goes along the raster width, `fy` along its height; swapped,
        // they would lay a non-square image with a skew.
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

    /// A quad from vertices in raster order. Longitudes are unwound toward the
    /// first vertex: otherwise a footprint across the antimeridian would be
    /// interpolated through the whole Earth.
    pub fn quad(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Quad(points.map(|(lat, lon)| (lat, geodesy::unwind(base, lon))))
    }

    /// Bounds of a complex outline -- with the same longitude unwinding, for
    /// the same reason.
    pub fn rough(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Rough(points.map(|(lat, lon)| (lat, geodesy::unwind(base, lon))))
    }

    /// Raster fraction (0..1 left to right and top to bottom) -> latitude and
    /// longitude.
    pub fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        match self {
            Self::Projected { system, affine } => {
                let x = affine[0] * fx + affine[1] * fy + affine[2];
                let y = affine[3] * fx + affine[4] * fy + affine[5];
                // There is nothing to unwind the longitude with here, and no
                // need: the inverse projection returns it as "central meridian
                // plus offset", that is already on a continuous branch, and a
                // raster of the edge zone past the seam gets a longitude above
                // 180° instead of jumping to -180°. A node could not do its own
                // unwinding anyway -- it is computed on its own and sees no
                // neighbour (cf. [`Grid::unwind`]).
                system.to_geodetic(x, y)
            }
            // Between the footprint corners there is nothing but the surface,
            // and the edge is led along it exactly as it was named
            // (`geodesy::edge_point`): matching latitudes mean a parallel,
            // matching longitudes a meridian, anything else the shortest arc.
            // The same rule outlines this very footprint (`outlines::edge`) and
            // catches the click (`footprint::traced`), and it cannot be asked
            // about separately: a raster laid by an arc along an edge that was
            // outlined as a parallel misses its own outline by a hundred metres
            // at a one-degree cell.
            //
            // The price is named where the rule is: the kind of edge is decided
            // by coordinate coincidence, and the catalogue hands them out
            // rounded. Should two corner latitudes coincide by chance, the
            // imaging strip lies along a parallel instead of an arc -- that is,
            // off by kilometres. But it is off the same way: the outline and
            // the click read the footprint with one rule, the raster with its
            // own, and only the raster diverged from them.
            Self::Quad([ul, ur, lr, ll]) | Self::Rough([ul, ur, lr, ll]) => {
                let top = geodesy::edge_point(*ul, *ur, fx);
                let bottom = geodesy::edge_point(*ll, *lr, fx);
                // The kind of vertical run is asked of the computed ends, not
                // the side edges: the meridian branch returns the longitude of
                // the start, and taken from the side edge it would drag the
                // bottom edge of the raster away from its own corner -- tens of
                // kilometres on a slanted quad.
                geodesy::edge_point(top, bottom, fy)
            }
            Self::Grid(grid) => grid.geodetic(fx, fy),
        }
    }

    /// Metres of ground per pixel of a raster `width` pixels wide. For a quad
    /// and a grid -- across the widest of the three rows and by the coarse
    /// degree metric: this is asked in order to pick the pyramid level, and
    /// that changes twofold per step.
    ///
    /// Three rows, not the top one: on a polar raster the top row degenerates
    /// into a point, and measuring by it understates by the cosine of its
    /// latitude -- fivefold at the eightieth parallel, and sixteen quadrillion
    /// times at the pole itself (`cos(90°)` in `f64` is 6·10⁻¹⁷ rather than
    /// zero, so no error even comes out). Understated, it drives `sharpest`
    /// down so many levels that it hits the top of the pyramid: an image of the
    /// whole Earth is shown at one tile across and never gets sharper, without
    /// a single failure.
    ///
    /// The same degeneration can bite both edges at once -- on a frame from
    /// pole to pole -- and only the middle saves it. It does not save a frame a
    /// full turn wide: `Frame::quad` unwinds longitudes toward the first vertex
    /// without guarding the full turn, and such a frame folds into a meridian
    /// even before any measuring. Circular outlines never reach it, though --
    /// wider than 350° the caller sends them to bounds.
    ///
    /// The widest, not the average: an understated measure leaves part of the
    /// raster blurry forever, while an overstated one is coarsened by the
    /// appetite ceiling (`tiles::want`). An error one way is fixable, the other
    /// way it is not.
    ///
    /// This is asked of the binding itself, not of the grid nodes: a global
    /// degree raster has only four nodes, sitting at the corners, that is at
    /// the poles -- there is no middle among them at all.
    ///
    /// `None` -- the frame cannot be measured, and that is not "very fine" but
    /// "no width at all": this is how a quad with coinciding vertices lies.
    /// Returned as a number, zero divides level selection by itself --
    /// `sharpest` goes to `u32::MAX`, the layer stays at the top of the pyramid
    /// forever, and the "preview suffices" comparison becomes eternally true,
    /// and the detailed raster is never requested.
    ///
    /// For a projection -- the length of the vector by which the raster width
    /// lies on the zone, not its eastern component alone: a rotated raster
    /// would then report the cosine share of its width and land a level off.
    /// The length is also non-negative by itself, and which frame edge is more
    /// to the left the contract does not specify -- a negative width would give
    /// `log2` of a negative, that is `NaN`, that is the zero level outright.
    pub fn ground_m_per_px(&self, width: u32) -> Option<f64> {
        // Three rows -- fractions 0, 0.5 and 1 down the height. By links, not by
        // the chord between the ends: between probes a row can bulge, and the
        // chord loses that bulge. For a grid its widest row by nodes is added to
        // the probes: without it an accepted `Grid::new` grid would stop being
        // guaranteed measurable -- the gates there count by nodes, while a probe
        // takes fractions, and a wide row between fractions they would see
        // differently.
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

    /// Do the frame rows run along parallels and the columns along meridians.
    ///
    /// For such a frame latitude depends only on the vertical fraction and
    /// longitude only on the horizontal one, and the warp-mesh nodes are
    /// computed by row and by column rather than each on its own (see
    /// [`nodes_of`]). This is how every raster in degrees lies; an acquisition
    /// with its own bent grid and a projected raster are not like that.
    pub fn axial(&self) -> bool {
        match self {
            // Web Mercator does not disturb the axes: its longitude depends
            // only on the eastern coordinate and latitude only on the northern
            // one. The frame lies along the axes when its affine transform
            // equally does not disturb them -- that is, for an axis-aligned
            // raster, without rotation or skew.
            Self::Projected { system: projection::Projection::WebMercator, affine } => {
                affine[1] == 0.0 && affine[3] == 0.0
            }
            // The transverse one does: on the UTM metre grid latitude changes
            // along a row, and longitude along a column.
            Self::Projected { .. } => false,
            // For a quad the run along an edge is decided by coordinate
            // coincidence (`geodesy::edge_point`): matched latitudes -- the
            // edge is led by a parallel, matched longitudes -- by a meridian.
            // When both match, the rows and columns of the quad are precisely
            // parallels and meridians.
            Self::Quad([ul, ur, lr, ll]) | Self::Rough([ul, ur, lr, ll]) => {
                ul.0 == ur.0 && ll.0 == lr.0 && ul.1 == ll.1 && ur.1 == lr.1
            }
            Self::Grid(grid) => grid.axial,
        }
    }

    /// Whether the frame has any extent on Earth -- that is, whether one can
    /// pick a level and draw a cell by it at all.
    pub fn measurable(&self) -> bool {
        self.ground_m_per_px(1).is_some()
    }
}

/// Tie point grid: nodes stand on a grid of raster fractions, and the display
/// interpolates between them.
///
/// A grid, not four corners: a radar image lies in the acquisition geometry,
/// and a quad does not describe it -- curvature accumulates in kilometres. The
/// file carries the grid (21x21 for a Sentinel-1 granule), and it also answers
/// the question of the raster's rotation angle: the footprint quad has its own
/// vertex order and is not obliged to match the raster order.
pub struct Grid {
    /// Raster fractions at which the nodes stand, by axes -- increasing.
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Nodes row by row (ys x xs): latitude and longitude. Longitudes are
    /// unwound into a continuous run along the grid itself -- see
    /// [`Grid::new`].
    nodes: Vec<(f64, f64)>,
    /// Whether the grid is axial: every row of it shares one latitude and
    /// every column shares one longitude. This is how every raster in degrees
    /// lies -- from granule to global -- and neither an acquisition (its grid
    /// bends along the track) nor a projected raster lies this way.
    ///
    /// This is asked for the sake of the warp-mesh nodes: on an axial frame
    /// latitude does not depend on longitude and vice versa, and the
    /// trigonometry is computed once per row and once per column instead of
    /// once per node (see [`nodes_of`]).
    ///
    /// Computed at assembly: the grid does not change afterwards.
    axial: bool,
    /// The longest row of the grid on Earth, metres (see [`Grid::widest_row`]).
    ///
    /// Computed once, at assembly: the grid does not change afterwards -- the
    /// fields are closed and its only `&mut self` is the unwinding, and it is
    /// called from there as well. This quantity is asked on the frame path,
    /// through [`Frame::ground_m_per_px`], several times per moving frame,
    /// while it costs a walk over all grid links with a square root on each.
    widest_row_m: f64,
}

/// How far nodes may diverge while staying on one grid line.
///
/// Raster fractions are numbers of order one, and they come from dividing a
/// pixel by the side, so they can diverge only in the last digits. A millionth
/// is less than a pixel of a hundred-thousand-point raster and much less than
/// the distance between lines: nodes stand no closer than one sample to each
/// other, that is no closer than a pixel fraction, and would merge only on a
/// raster a million wide.
const SAME_LINE: f64 = 1e-6;

/// The grid line number this fraction stands on.
fn line(axis: &[f64], at: f64) -> Option<usize> {
    let index = axis.partition_point(|line| *line < at - SAME_LINE);
    axis.get(index).filter(|line| (**line - at).abs() <= SAME_LINE).map(|_| index)
}

impl Grid {
    /// A grid from tie points. `None` -- the points do not form a grid: there
    /// are fewer than four, they are incomplete or scattered. There is nothing
    /// to guess here -- the binding stays as it was.
    ///
    /// Grid lines are recognized with a tolerance, not by exact equality:
    /// raster fractions come from dividing a pixel by the side, and on a file
    /// where a node is recorded even slightly its own way, exact comparison
    /// would shatter the grid into as many lines as it has nodes. The whole
    /// binding would be lost with it -- the image would lie by the catalogue
    /// quad, that is, most likely, rotated (see [`SAME_LINE`]).
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
        // After the unwinding: it moves longitudes, and an axial grid with a
        // seam inside becomes axial only once unwound.
        grid.axial = grid.axial();
        // After the unwinding, not before: for a row across the seam the raw
        // longitudes jump a full turn, and the width computed from them is
        // overstated several times -- by the grid from the test
        // `unwinding_does_not_care_where_the_zero_meridian_is` it is forty
        // thousand kilometres instead of eight hundred eighty.
        grid.widest_row_m = grid.widest_row();
        // A lattice of raster fractions is not yet a binding: a nadir
        // instrument has no extent across the track at all, and nodes laid out
        // by fractions into an honest grid fall on the ground in one line. The
        // quads of such a grid come out zero-area -- nothing is drawn -- while
        // dividing by its width pins the layer to the top of the pyramid. A
        // refusal here returns the image to the already written path "there are
        // no points, it lies by the catalogue outline".
        (grid.widest_row_m > 0.0 && grid.widest_column() > 0.0).then_some(grid)
    }

    /// Longitudes -- into a continuous run along the grid: each node is unwound
    /// toward its neighbour, not toward one common vertex.
    ///
    /// Unwinding toward a common vertex is not allowed: the grid of a global
    /// raster goes all 360°, and such unwinding would fold it into a meridian
    /// -- the opposite edge would end up half a metre from the first.
    /// Neighbouring grid nodes stand tens of kilometres apart, and the
    /// difference between them is never more than a half-turn -- so the seam
    /// between them is seen unambiguously.
    fn unwind(&mut self) {
        let across = self.xs.len();
        // A full turn stays a turn, and this case is exactly one -- a row of two
        // nodes. This is how a global raster lies, whose edge is recorded as
        // -180 -> 180: folded to the nearest branch it gives zero width -- the
        // very width later divided by when picking a level.
        //
        // For a row of three nodes and more the gates are neither needed nor
        // helpful. Not needed -- because a global row goes along it in half-turn
        // steps and the unwinding takes them by itself. Harmful -- because the
        // turn is recognized with a tolerance ([`geodesy::FULL_CIRCLE_DEG`])
        // while `previous` is already unwound by this place: on a polar row the
        // chain creeps past 180°, the difference from the raw node approaches a
        // turn by itself, and the node falls back to the raw value -- 359° from
        // its neighbour. The answer would then depend on where the file keeps
        // its zero longitude, and it must not.
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
            // The row is unwound within itself, but toward the previous one it
            // still has to be shifted -- as a whole, so the run inside it is
            // not broken.
            //
            // How much to shift is decided by all the columns, not the first:
            // on a pass running near the pole the first column is the polar
            // edge, and its longitude sweeps a full turn over fifty rows while
            // the far edge barely moves. A row shifted by such a column travels
            // a whole turn, and its far edge ends up 359° from the neighbour.
            //
            // Counted in turns, not degrees: the shift must stay a multiple of
            // a full turn, while the average of two disagreeing votes would
            // give a half-turn. Votes even -- the smallest in magnitude wins:
            // zero means "the rows are continuous as they are". Magnitudes
            // even -- the negative one wins; the choice is indifferent here,
            // what matters is only that it is the same from run to run.
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

    /// Whether every row shares one latitude and every column shares one
    /// longitude.
    ///
    /// By comparison to the last digit, not with a tolerance: on a raster in
    /// degrees the row corners are derived from one and the same number
    /// (`adapters::geo_ties`) and must coincide exactly. A tolerance would
    /// accept a slightly skewed grid as axial -- and nodes on it would land off
    /// the binding, and that could be seen only by eye.
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

    /// Whether the grid describes something a linear transform cannot.
    ///
    /// Four nodes are the corners, and between them exactly linear is
    /// interpolated -- the same thing a projection binding provides. Denser
    /// than four is already not: every extra node says the layout departs from
    /// linear, and that is what makes the grid valuable.
    pub fn is_dense(&self) -> bool {
        self.xs.len() * self.ys.len() > 4
    }

    /// The longest row of the grid on Earth, metres, stretched from its share
    /// of the width to one.
    ///
    /// Length by links, not by the chord from the first node to the last: the
    /// grid's longitudes are unwound along the grid itself ([`Grid::unwind`]),
    /// so a full-turn row honestly gives its 360°, while the row's bulge
    /// between nodes the chord loses.
    ///
    /// The longest, not the top: on a polar raster the top row degenerates into
    /// a point, and by it the grid would look like a line. Two ask about this --
    /// [`Grid::new`], to tell a grid from a line (the question there is exactly
    /// this: whether everything degenerated, not some first row), and
    /// [`Frame::ground_m_per_px`], so that an accepted grid stays measurable.
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

    /// The same by columns: the longest column of the grid on Earth.
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

/// An axis cell and the fraction inside it. Beyond the grid edge the fraction
/// goes outside [0, 1]: the outer nodes stand at the centers of the outer
/// pixels, while half a pixel beyond them must be shown too -- there the grid
/// continues with a straight line.
fn cell(axis: &[f64], at: f64) -> (usize, f64) {
    let last = axis.len() - 2;
    let i = axis.partition_point(|edge| *edge <= at).saturating_sub(1).min(last);
    (i, (at - axis[i]) / (axis[i + 1] - axis[i]))
}

/// Linear between grid nodes -- where walking an arc is pointless: nodes stand
/// twenty-odd kilometres apart, and the arc and the chord diverge by metres
/// (on a quad, where four hundred kilometres lie between corners, everything
/// is different).
fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Distance between points along the surface, metres -- on a sphere with the
/// semi-major axis: flattening gives a third of a percent, and this is asked
/// for picking the pyramid level, which changes twofold per step.
fn ground_span(from: (f64, f64), to: (f64, f64)) -> f64 {
    arc_deg(from, to).to_radians() * geodesy::SEMI_MAJOR_M
}

/// Angular distance between binding points, degrees.
///
/// Measured by the coordinates themselves, not by the chord of a sphere, and
/// the difference here is not about precision. Longitudes of the binding are
/// unwound along the binding itself (see [`Grid::unwind`] and [`Frame::quad`]),
/// so an edge of a full turn honestly gives 360° -- while the chord between
/// its ends is zero: its ends are one and the same point of the sphere. It was
/// exactly on this zero that a global raster lost everything at once: the
/// width by which the level is picked and the warp-mesh density.
///
/// Coarseness is enough: this is asked to pick the pyramid level and split the
/// cell, and both change twofold per step.
fn arc_deg(from: (f64, f64), to: (f64, f64)) -> f64 {
    let along = (to.1 - from.1) * ((from.0 + to.0) * 0.5).to_radians().cos();
    (to.0 - from.0).hypot(along)
}

/// A variable to the wire -- as is.
fn wire_variable(variable: &tiles::Variable) -> crate::proto::globe::Variable {
    crate::proto::globe::Variable { path: variable.path.clone(), said: variable.said.clone(), units: variable.units.clone() }
}

/// One overlay raster and all of its expectations.
pub struct Raster {
    pub role: Role,
    pub resource: veldsdk::OwnedResource,
    /// The file with the pixel coordinates of the raster, when they lie
    /// separately from it (Sentinel-3). Held exactly so that it is released
    /// together with the raster: it is asked for once, at description.
    pub geolocation: Option<veldsdk::OwnedResource>,
    /// The raster number at the sender (`OverlayRaster.ordinal`): with it the
    /// sender learns which file lies as the detailed one -- there are no names
    /// here.
    pub ordinal: u32,
    /// The variable of this file named by the sender (`OverlayRaster.variable`);
    /// empty -- the tiler chooses. It travels into the description and into
    /// every pass. For a spare it is empty: that is another file, and what was
    /// named about the previous one does not apply to it.
    pub variable: String,
    /// The file's variables to choose from -- from the last description that
    /// named them (`Meta.variables`). They outlive a refused description: one
    /// refused over a named variable carries no list, while picking another is
    /// needed exactly then.
    pub variables: Vec<tiles::Variable>,
    /// Spare files of the same role -- number, resource and coordinates, in
    /// order of preference: the provider names them after the chosen one when
    /// the choice by names may err (the SLSTR visible channel empty at night),
    /// and the next one takes the place of the one that failed to describe
    /// ([`Raster::next_spare`]). Released together with the raster.
    pub spares: Vec<(u32, veldsdk::OwnedResource, Option<veldsdk::OwnedResource>)>,
    /// Raster description. Assigned only through [`Raster::describe_as`]: with
    /// the sizes the warp-mesh nodes change too, and their cache would not
    /// learn about it otherwise.
    pub meta: Option<Meta>,
    /// What the layer will lack because of this raster, even though the layer
    /// itself landed fine. Alongside `Overlay::error`, but a different answer --
    /// "it will not get sharper" versus "there is nothing to look at" -- and
    /// the layer stays on the globe with it.
    ///
    /// On the raster, not on the layer: there can be two reasons, one per
    /// raster, and the second does not cancel the first. Piled into one field
    /// they would cancel each other in description order -- the one that
    /// described successfully would erase its neighbour's complaint.
    pub trouble: Option<String>,
    pub describe: veldsdk::Latest,
    /// What has already been requested, what is being produced and what must
    /// not be requested again -- the shared tile consumer accounting (see
    /// `veldmap_image_tiler_wrap::tiles`).
    pub fetch: Fetch,
    /// Cell balls by levels, from the zeroth to the top (see
    /// [`Overlay::bounds`]).
    ///
    /// Built from the binding and the raster sizes and independent of the view
    /// at all, so they are computed once per description, not per frame. Empty
    /// -- no description yet: then there is no culling and every cell goes to
    /// the exact test.
    pub bounds: Vec<Vec<cull::Ball>>,
    /// Progress of the running producer pass: source bytes read out of how
    /// many. Zero denominator -- there is no pass.
    ///
    /// Stored because a level can be a single tile, and to give it out, a
    /// sequential source is read whole: by cells exactly zero is done all that
    /// time, and there is nothing else to say about the work.
    pub pass: (u64, u64),
    /// Warp-mesh nodes by cells -- the dearest thing in a patch and the one
    /// that changes most rarely (see [`Nodes`]).
    ///
    /// On the raster, not on the layer: a node's place is derived from the
    /// layer's binding **and** the raster's sizes, and the layer has two
    /// rasters with different sizes. Only the cells of the last rebuild are
    /// held here -- the rest fall out by themselves (see [`patches`]).
    mesh: std::collections::HashMap<tiles::Addr, Nodes>,
    /// The layer relay at which the nodes were computed. If it diverges from
    /// `Overlay::relaid` -- the mesh stands by the old frame, and it must be
    /// recomputed.
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

    /// The next spare file -- into the place of the one that failed to
    /// describe: the raster stays the same (role, place in the overlay), while
    /// everything it knew about the old file starts from a clean slate. The
    /// answer is the new file's resources for description; `None` -- no spares
    /// left. The previous file is released right here, together with its
    /// coordinates.
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

    /// Refused description. With a named variable -- a complaint in the
    /// tiler's words, the file stays: it was not the file that was refused but
    /// the choice, and the sender will name another one -- a spare does not
    /// take its place. Without a named one -- the next spare file takes the
    /// place (the answer is its resources for description), and with no spares
    /// left -- a complaint about the raster, named by role.
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

    /// Accept the raster description.
    ///
    /// A method, not field assignment: the warp-mesh nodes are computed from
    /// the raster sizes, and what was computed for the old ones does not serve
    /// -- while the cell addresses of the new description are the same, so the
    /// divergence is invisible by them.
    ///
    /// Today there is nothing to clean: a raster receives a description once
    /// in its life (`describe` is created in `adopt_overlay`, and for a spare
    /// in `describe_settled`, on a raster started from a clean slate), and
    /// before that the cache is empty. The rule stands here so that a second
    /// description does not become a quiet breakage.
    pub fn describe_as(&mut self, meta: Meta) {
        if !meta.variables.is_empty() {
            self.variables = meta.variables.clone();
        }
        self.meta = Some(meta);
        self.mesh.clear();
    }

    /// Keep exactly these cells in the node cache -- and only if they were
    /// computed at this same layer relay.
    ///
    /// A relay cancels the computed ones entirely: the nodes stand by the old
    /// frame, while the cell addresses of the new one are the same -- the
    /// divergence is invisible by them.
    ///
    /// The rest fall out because otherwise the cache would grow on everything
    /// that ever entered the frame: a cell of a global raster is nodes by the
    /// density of its warp mesh (`segments_for`), each the size of
    /// `OverlayVertex`, and so on every pyramid level.
    /// `cells` -- in ascending order: every cache cell is searched among them,
    /// and a linear search would give a square in the cell count. At the
    /// current ceiling that is thousands of comparisons, and the ceiling is
    /// set by the video memory budget from the config and grows with it.
    fn keep_mesh(&mut self, relaid: u64, cells: &[tiles::Addr]) {
        if self.meshed != relaid {
            self.mesh.clear();
            self.meshed = relaid;
        }
        self.mesh.retain(|addr, _| cells.binary_search(addr).is_ok());
    }

    /// The fraction read by the running pass, 0..1. Zero -- there is no pass
    /// or it has just started.
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
    /// How many times the layer was relayed to another binding.
    ///
    /// A counter, not the frame itself: it does not enter the fingerprint of
    /// the drawn ([`Overlay::built`]) -- comparing a grid of four hundred nodes
    /// every frame costs more than assembling the vertices anew -- while a
    /// relay must be noticed.
    pub relaid: u64,
    /// How the frame was obtained. Next to it, not derived from its kind: a
    /// projection frame comes both from the MGRS layout in the product name and
    /// from the file's own binding -- by the `Frame` variant they are
    /// indistinguishable (see [`Binding`]).
    pub binding: Binding,
    /// Why the binding from the raster was not taken although the raster
    /// carried it.
    ///
    /// On the layer, not on the raster: the image has one binding, and the
    /// failure of one raster is cancelled by the success of another -- while a
    /// complaint pinned to a raster would outlive that success and lie. It
    /// leaves together with the raster reasons ([`Overlay::trouble`]), because
    /// for the viewer this is one question: what the layer will lack.
    pub binding_trouble: Option<String>,
    pub rasters: Vec<Raster>,
    /// The resources the overlay was sent with -- exactly as they arrived. By
    /// them it is decided "the same overlay or another": comparing against the
    /// accepted ones is not possible, because fewer can be accepted than sent
    /// (a raster refused a grant is skipped), and then the set would diverge
    /// from itself forever -- every resend would look like a new overlay.
    pub sources: Vec<u64>,
    /// Named variables of the sent rasters, one per raster in sending order
    /// (`OverlayRaster.variable`). The second half of the set identity: the
    /// same resources with another variable -- the same overlay, but the
    /// raster with it is described anew ([`Overlay::revariabled`]).
    pub variables: Vec<String>,
    /// Layer opacity, 0..1. Reaches the pixel as a multiplier on the carrier's
    /// alpha, so the transparent field of a quicklook stays transparent
    /// instead of showing through at half strength.
    pub opacity: f32,
    /// The layer is hidden: no patches, no tile requests. Already fetched tiles
    /// stay in the store at the mercy of eviction -- showing it back is then
    /// instant, and paying memory for them only lasts until they are evicted.
    pub hidden: bool,
    /// Why the layer will not lie: not a single raster described. Empty -- all
    /// is well. The reason travels to the sender along the fetch progress:
    /// removing the overlay is their business, the set belongs to them (see
    /// `OverlayProgress.error`).
    pub error: String,
    /// Fetch progress as computed by the last frame (see [`Progress`]).
    ///
    /// Stored rather than derived in place: it can only be derived with a live
    /// view -- the step is computed by the frame matrix -- while the layer must
    /// be told about even when the globe tab is closed. A silently frozen
    /// counter at the receiver looks like a hung download.
    pub progress: Progress,
}

/// Raster-to-projection binding as read by the tiler -- with the sizes of the
/// raster it was taken from.
///
/// Fields, not a six of numbers: the numbers arrive from a message where they
/// carry the same names, and rewritten by names they converge by themselves. A
/// positional list would also converge with a swapped pair -- and a swapped
/// pair lays the image skewed, and by the look of the frame that is
/// indistinguishable from an honest rotation.
pub struct Placement {
    pub epsg: u32,
    /// Raster pixel -> system metres:
    /// `x = x_per_i·i + x_per_j·j + x0`, `y = y_per_i·i + y_per_j·j + y0`.
    pub x_per_i: f64,
    pub x_per_j: f64,
    pub x0: f64,
    pub y_per_i: f64,
    pub y_per_j: f64,
    pub y0: f64,
    /// The sizes of the raster whose pixels name the transform: the frame
    /// lives in fractions, and the overlay has two rasters of different sizes.
    pub width: u32,
    pub height: u32,
}

/// Where the layer's binding came from -- and hence how much to trust it.
///
/// Precedence by kind, not by arrival. An overlay has two rasters, preview and
/// detailed, and they are described independently: which one describes first
/// is decided by the parse time of two files, that is by nothing meaningful.
/// The rule "the one that arrived first stays" serves while both bindings are
/// of one kind -- then they are about the same image -- and lies as soon as
/// the kinds differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    /// Catalogue outline: it says which piece of Earth was shot, not which
    /// pixel goes where, and its vertex order is not obliged to match the
    /// raster order.
    Catalogue,
    /// The zone frame derived by the provider from the product name (MGRS).
    /// Exact, but promised by the name rather than stated by the file: the
    /// product layout may diverge from what is recorded in the raster.
    Named,
    /// The projection recorded in the raster itself: a linear layout, and that
    /// is all there is to say about it.
    Projected,
    /// The tie point grid from the raster, denser than four nodes. Stronger
    /// than the projection, and not by the prestige of the source: such a grid
    /// describes a **nonlinear** layout -- a radar image lies in the
    /// acquisition geometry -- while a six of numbers is linear by definition.
    /// Substituting it with the projection would be lying exactly about what
    /// the projection is rejected for in the tiler.
    ///
    /// A grid of exactly four nodes describes nothing nonlinear -- between
    /// them the same linear is interpolated -- and does not become stronger
    /// than the projection (see [`Grid::is_dense`]).
    Lattice,
}

/// Fetch progress of a single layer.
///
/// `share` is the layer's fraction of the way, and it is counted by pyramid
/// levels, not by frame cells: there are as many levels as there are and each
/// closes once, while visible cells are as many as have just entered the
/// frame. A bar derived from the latter would ride back and forth on every
/// camera move, reporting nothing.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Progress {
    /// Cells of the current order done and how many are in it. The order, not
    /// the frame: see `tiles::Fetch::ordered`.
    pub ready: u32,
    pub total: u32,
    pub share: f32,
    /// Which rung of the ladder is being filled and how many there are in
    /// total -- what the fraction counts. Travels out with it: the fraction
    /// without them is a number without a unit.
    pub step: u32,
    pub steps: u32,
    /// The fraction within the current level, 0..1 -- the same thing `share` is
    /// computed from, but without dividing by the ladder.
    ///
    /// Held separately because it is not asked for the bar: it measures the
    /// ripple strength on the loading cells, and the ladder has nothing to do
    /// with it -- the level ripples, not the way to the goal.
    pub within: f32,
    /// Pass bytes fit for display (see `tiles::Inside::read`).
    pub pass: (u64, u64),
}

/// Layer fetch progress by the raster currently drawn and the level being
/// climbed to.
///
/// A pure function and here rather than in the handler: it is computed from
/// two numbers and one rule, and there is nothing else to check it with --
/// `module.rs` has no tests at all, and cannot have them while everything in
/// it touches the whole state.
pub fn progress_of(raster: &Raster, want: &tiles::Want) -> Progress {
    let (ready, total) = raster.fetch.ordered();
    // Whether EXACTLY THIS level is read pointwise: for a strip source the
    // detailed one goes by a window while the coarse one goes by a pass, and
    // bytes are honest only for the latter.
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

/// What to draw on the overlay right now: which raster and at what level.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub role: Role,
    pub fingerprint: String,
    pub level: u32,
}

/// What the overlay's vertices are assembled from. If it matches for all --
/// there is nothing to reassemble.
///
/// With named fields, not a tuple: a forgotten field here is not a compiler
/// error but an image left lying by the old binding or at the old place of the
/// frame -- and that can be seen only by eye.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    key: String,
    opacity: f32,
    relaid: u64,
    choice: Choice,
    /// Cells for which a carrier was found: the cell itself, what covers it and
    /// which bind group that texture has.
    ///
    /// By carriers, not just cells: an arrived tile changes the cover of one
    /// cell, while the store generation moves every arrival and every
    /// eviction, including foreign ones. Measured by the generation, the
    /// fingerprint would call for reassembling everything on every tile;
    /// measured by the carriers, it calls exactly when the drawn would indeed
    /// become different.
    ///
    /// The bind group is here because the tile moves: having arrived anew, it
    /// lands in a new texture, and its `uv` and binding differ while the
    /// address is the same. This relies on the host not reusing resource ids
    /// (`ResourceRegistry::register`): a matched number -- the same carrier.
    cells: Vec<Drawn>,
}

/// The same along with what can be seen from it: cells of the chosen level
/// that entered the frame. As one value because they are computed together and
/// separately make no sense -- the level is picked precisely by how many of
/// its cells are visible.
pub struct Wanted {
    pub choice: Choice,
    /// The rung toward the goal and its cells -- the answer of the shared
    /// ladder, in full.
    ///
    /// In full, not field by field: the layer's fetch progress is measured in
    /// levels (`climbed` of `steps`), and their denominator is not the number
    /// of levels but the length of the ladder -- for a source where a level
    /// costs a whole pass there are two for the entire pyramid. Having split
    /// them here into their own fields, we would create a second copy of the
    /// same count, diverging from the one "work is going" is computed by
    /// (`tiles::working`).
    pub want: tiles::Want,
}

/// The view by which the desired is measured: what to project with, where the
/// eye is and how much ground falls per frame pixel. As one thing -- they are
/// always asked together and always about one frame.
///
/// Compared as one because that is also what guards against needless
/// recomputation: a key assembled from the view's arguments one by one will
/// sooner or later miss one of them, while a comparison of the view itself
/// cannot miss anything.
#[derive(Clone, Copy, PartialEq)]
pub struct Look {
    pub view_proj: camera::Mat4,
    pub eye: World,
    pub mpp: f64,
}

impl Overlay {
    /// What the layer will lack although it has landed -- about all rasters at
    /// once.
    ///
    /// Assembled at the time of display rather than stored: there can be two
    /// causes, and stacked into one field they would cancel each other by the
    /// order of description. Empty -- there is nothing to say.
    ///
    pub fn trouble(&self) -> String {
        let said: Vec<String> = self
            .rasters
            .iter()
            .filter_map(|raster| raster.trouble.clone())
            .chain(self.binding_trouble.clone())
            .collect();
        said.join("; ")
    }

    /// How the layer looks from the outside. Here rather than at the dispatch,
    /// because only this is verifiable: the dispatch publishes, and publication
    /// in the native build is a stub (`veldsdk::abi`), so what is sent is
    /// invisible to the test.
    ///
    /// `mine` -- what this layer wants in the current frame; `None` means it
    /// does not ask for a frame at all (hidden, no room, the recomputation has
    /// not reached it yet).
    pub fn report<K: PartialEq>(
        &self,
        mine: Option<&Wanted>,
        passes: &Passes<K>,
    ) -> crate::proto::globe::OverlayProgress {
        let live = mine.is_some();
        let working = self.working(passes, mine, live);
        // The frame draws **the very** raster whose cap we name, and there is
        // no work behind it -- only then does the cap answer the question
        // asked (see [`Overlay::said`]). The role in this pair is not a
        // quibble: with Sentinel-2 the frame has reached the preview and draws
        // it while the detailed raster has only just been described -- and
        // "it will not get sharper" about the detailed raster at that instant
        // speaks of what is not yet on the screen at all.
        //
        // There is deliberately no separate "layer in frame" flag beside it: a
        // layer that does not ask for a frame has no role, and it has nothing
        // to match -- except the case where the deciding raster is missing
        // too. And without it there is no cap, so the `said` answer does not
        // depend on this at all.
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

    /// The same for the wire -- together with the detail cap
    /// (`Meta::capped`).
    ///
    /// Kept apart from [`Overlay::trouble`] because the cap is not always in
    /// place: a complaint is about what has happened and is true from the
    /// second it happened, while the cap answers "it will not get any sharper"
    /// and over an unfinished layer reads as "the fetch is over, here is its
    /// ceiling".
    ///
    /// `settled` -- the layer is showing and there is no work behind it. Both
    /// halves are needed, and the second alone is not enough: right after the
    /// description the recomputation of the wanted has not run yet, the layer
    /// has not made it into the set, and [`Overlay::working`] honestly answers
    /// "no" -- with one percent of the raster read. The canvas computes
    /// `settled` the same way and for the same reason (`View::said`).
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

    /// The same, split by whom it is about: (about the detailed raster,
    /// everything else). About the detailed -- its failure and the detail cap
    /// when the detail is decided by it; the rest -- the preview failure, the
    /// preview cap, the binding failure. The sender needs this boundary: it
    /// puts the file name before the words about it, and a preview cap or a
    /// binding failure under the detailed file's name would read as said about
    /// it.
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

    /// The raster that decides the layer's detail: the most detailed of the
    /// ones that get laid down.
    ///
    /// Not just any: the preview is always coarser than the detailed raster --
    /// otherwise the detailed one is eclipsed ([`Overlay::detail_eclipsed`]) --
    /// and speaking of the preview's cap would mean promising blur where
    /// zooming removes it. With Sentinel-2 both rasters are JP2, and both can
    /// have a cap, so this case is not made up.
    ///
    /// Counted over [`Overlay::budgeted`], not over all rasters: an eclipsed
    /// detailed raster is not drawn, and the detail is not decided by it. By
    /// the same token the answer is correct both when there is no detailed
    /// raster at all and when it is eclipsed -- in both cases the preview
    /// decides the detail.
    fn decider(&self) -> Option<&Raster> {
        self.budgeted()
            .filter(|raster| raster.meta.is_some())
            .max_by_key(|raster| raster.meta.as_ref().expect("selected by presence").reachable().0)
    }

    /// The detail cap of the **layer** -- the cap of the deciding raster, and
    /// `None` if it gives up its resolution entirely.
    fn detail_cap(&self) -> Option<String> {
        self.decider()?.meta.as_ref()?.capped()
    }

    /// The layer has nothing to lie on and nothing to wait for: no raster is
    /// described, and none has a named variable. A failure of the named
    /// variable does not bury the layer: a raster without a description but
    /// with its own list still lets the sender name another one -- a removed
    /// layer cannot offer that.
    pub fn doomed(&self) -> bool {
        !self.rasters.iter().any(|raster| raster.meta.is_some())
            && self.rasters.iter().all(|raster| raster.variable.is_empty())
    }

    /// The laid rasters for which the sender named a different variable than
    /// the one they are described with: `wanted` -- the variable by resource,
    /// as in the sent set. Spares do not count: the name refers to the file
    /// that lies here, not to the ones behind it. The answer is the roles
    /// whose rasters are to be described again.
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

    /// The overlay still has something to wait for. The description in flight
    /// is its own; everything else is counted by the shared rule
    /// (`tiles::working`) -- the same one the canvas answers this question by.
    ///
    /// `wanted` -- what the layer needs right now, per raster; `live` -- whether
    /// the layer is allowed to count at all (not hidden, there is room for a
    /// frame). Fetching does not run without this, and saying it "is running"
    /// would be a lie.
    ///
    /// The description lives by another rule and is asked before both: it is
    /// already in flight, does not ask for a frame and finishes on its own. For
    /// a raster that lies over the network it is the longest part of the path
    /// -- tens of seconds -- and, were it declared idle, it would leave the
    /// person who clicked the icon without a single sign of work.
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

    /// There is nothing to draw from the layer yet.
    ///
    /// Two causes, and both about one thing: no raster is described -- meaning
    /// there are neither cells nor size -- or the binding can still change, and
    /// then the layer is deliberately left undrawn (see
    /// [`Self::binding_pending`]). The second cause is not superfluous: with two
    /// rasters the preview is described first, but drawing from it is still
    /// impossible while the detailed one may bring the grid.
    ///
    /// The sender asks this -- by it it draws both the bar without a share and
    /// the place of the image on the globe (see `OverlayProgress.blank`): for
    /// these seconds there is nothing more to say about the image, and for a
    /// raster over the network they last tens of seconds.
    pub fn blank(&self) -> bool {
        self.binding_pending() || !self.rasters.iter().any(|raster| raster.meta.is_some())
    }

    /// The binding can still change, and so the overlay is neither drawn nor
    /// asked for tiles under it.
    ///
    /// The footprint quad is a guess at the angle the raster landed at on the
    /// captured piece of Earth, while the raster's description can bring a grid
    /// from the file itself. Drawing by the guess means showing the image
    /// rotated and turning it before the eyes a second later; asking for tiles
    /// by it means asking for the wrong cells: visibility is counted by the
    /// same binding. The UTM frame is not a guess, and it has nothing to wait
    /// for.
    ///
    /// The bounds of a complex contour ([`Frame::Rough`]) always wait: it is
    /// not a guess even approximately -- stretching the acquisition strip along
    /// it means showing it twice as wide as captured and at the wrong angle. If
    /// it does not wait, the layer ends in an error (see `module::on_described`)
    /// rather than a crooked picture.
    fn binding_pending(&self) -> bool {
        match self.frame {
            Frame::Rough(_) => true,
            Frame::Quad(_) => self.rasters.iter().any(|raster| raster.describe.is_pending()),
            Frame::Projected { .. } | Frame::Grid(_) => false,
        }
    }

    /// What to draw and what to want for the current view: the preview base and
    /// the detailed raster -- when the screen is smaller than the base's native
    /// resolution and when the detailed raster really is more detailed than it
    /// (see [`Overlay::detail_eclipsed`]). The order is the drawing order: the
    /// base is always underneath, the target on top -- this way the image is
    /// visible at first glance, and detailed tiles cover it as they arrive (a
    /// cheap start from the phase design).
    ///
    /// One answer for both askers -- the tile request and the patch assembly:
    /// the ordered level must be the same one that is drawn, and a cell's
    /// visibility by the same one it was asked by.
    pub fn wanted(&self, look: &Look, cap_tiles: u64, store: &Store, toll: &Toll) -> Vec<Wanted> {
        if self.binding_pending() {
            return Vec::new();
        }
        let mut wanted = Vec::new();
        let described = |role: Role| {
            let raster = self.raster(role)?;
            Some((raster, raster.meta.as_ref()?))
        };

        // An immeasurable frame is asked about nothing: choosing a level by it
        // is division by zero. Such a layer is already named erroneous at
        // description time (see `module::on_described`), but is not removed from
        // the set -- removing the layer is the sender's business -- and it
        // reaches here every frame.
        if !self.frame.measurable() {
            return Vec::new();
        }
        let base = described(Role::Preview);
        if let Some((raster, meta)) = base {
            wanted.push(self.at_level(raster, meta, look, cap_tiles, store, toll));
            if self.frame.ground_m_per_px(meta.width).is_some_and(|mpp| look.mpp >= mpp) {
                // The preview's native resolution is enough -- the detailed
                // raster is not needed.
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

    /// The detailed raster is no more detailed than the base -- laying it down
    /// is pointless and harmful.
    ///
    /// It is drawn on top (the list order is the drawing order), and zooming,
    /// on reaching it, would cover the big picture with a small one. The role is
    /// assigned by the provider from the product layout, and a layout it does
    /// not know it sorts out by comparing names -- and whatever happens to come
    /// turns out detailed.
    ///
    /// Three ask about this: the raster choice, the pyramid count in the video
    /// memory budget, and the one who tells the human what the layer will lack.
    /// Were they to diverge -- the raster would either eat the budget without
    /// drawing, or vanish silently.
    pub fn detail_eclipsed(&self) -> bool {
        let width = |role| {
            let meta = self.raster(role)?.meta.as_ref()?;
            // Reachable resolution rather than the recorded width: no matter
            // how many pixels the source has, it will give no finer than its
            // detail cap (`Meta::finest`). `Meta::reachable` counts it -- the
            // same answer that goes to the log and to the layer's caption.
            Some(meta.reachable().0)
        };
        match (width(Role::Preview), width(Role::Detailed)) {
            (Some(base), Some(detail)) => detail <= base,
            // Nothing to compare against -- so nothing to eclipse.
            _ => false,
        }
    }

    /// The rasters sharing the video memory budget: all but the eclipsed
    /// detailed one.
    ///
    /// The eclipsed one is not drawn, and it has no business sharing the
    /// budget: the cell ceiling is counted for the pyramid, and a spare one
    /// would coarsen the target for the one that stays -- that is, it would
    /// bring back the very blur the raster was eclipsed for.
    pub fn budgeted(&self) -> impl Iterator<Item = &Raster> {
        let eclipsed = self.detail_eclipsed();
        self.rasters.iter().filter(move |raster| !(eclipsed && raster.role == Role::Detailed))
    }

    /// The fingerprint of what is drawn: everything the vertices of this
    /// overlay are assembled from.
    ///
    /// It decides whether the geometry is reassembled. The cells are not here
    /// for completeness: any camera movement changes their set, while the
    /// raster's fingerprint with the level stays the same -- and without them a
    /// pan over the already fetched would leave the vertices at the previous
    /// position. The binding enters as the relaying counter (see
    /// [`Overlay::relaid`]), the opacity by itself: it is baked into the
    /// vertex, and otherwise the slider would not redraw the image.
    pub fn built(&self, wanted: &Wanted, drawn: &[Drawn]) -> Built {
        Built {
            key: self.key.clone(),
            opacity: self.opacity,
            relaid: self.relaid,
            choice: wanted.choice.clone(),
            cells: drawn.to_vec(),
        }
    }

    /// Relays the layer onto another binding.
    ///
    /// As a method rather than field assignment: the relaying counter must
    /// move along with the frame, otherwise the geometry reassembly will not
    /// notice it.
    pub fn relay(&mut self, frame: Frame, binding: Binding) {
        self.frame = frame;
        self.binding = binding;
        self.relaid += 1;
        // The binding is taken -- there is nothing to complain about: the
        // complaint here is always about one thing, that taking it failed. It
        // is cleared next to the frame, not by the caller: three places set it,
        // and a single clearing place left to their memory would diverge from
        // them by the order of descriptions.
        self.binding_trouble = None;
    }

    /// Why the binding could not be taken from the raster -- what the viewer
    /// will see.
    ///
    /// The layer is asked, not the raster: there are two rasters, they lie on
    /// one binding, and one's failure means nothing when the second has already
    /// landed as read. Hence the rank guard -- while nobody has brought a read
    /// one, the complaint is needed; one brings it, and the complaint is stale.
    ///
    /// The order of descriptions is arbitrary, and the outcome does not depend
    /// on it: a success arriving later clears the complaint
    /// ([`Overlay::relay`]), a complaint arriving later is not added to the
    /// success.
    ///
    /// Causes accumulate rather than crowd each other out: with two rasters
    /// they are different, and the second does not cancel the first. The same
    /// one, however, is not repeated -- with two files of one product the
    /// trouble is more often than not one and the same, and said twice it reads
    /// as two.
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

    /// The rung of fetching under the view and the visible cells of that rung.
    ///
    /// The shared rule counts it (`tiles::want`) -- the same one by which the
    /// viewing canvas picks the level: both the target per screen pixel with
    /// the appetite ceiling, and the rung toward it. The overlay brings one
    /// thing of its own there -- `sharpest` (which level answers a frame pixel
    /// at this binding) and the answer to "what of the level is visible".
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

    /// The level whose pixel is no larger than the screen's -- the one thing
    /// about level choice the overlay knows and the canvas does not: for a
    /// raster on the globe the scale is set by the binding, not by a camera
    /// over the picture.
    fn sharpest(&self, meta: &Meta, look: &Look) -> u32 {
        // `wanted` does not let a frame without width in here; zero here is
        // the most detailed level, that is the most expensive answer to a
        // question nobody asked.
        let Some(mpp_raster) = self.frame.ground_m_per_px(meta.width) else { return 0 };
        match look.mpp <= mpp_raster {
            true => 0,
            false => (look.mpp / mpp_raster).log2().floor() as u32,
        }
    }

    /// Balls covering the raster's cells -- one set per level, from zero to the
    /// top.
    ///
    /// They cut a cell off before the exact test: a ball against the frame
    /// costs three dozen additions, while nine probes cost six hundred
    /// transcendental calls (see [`crate::cull`]).
    ///
    /// Each level is probed with its own nine points rather than derived from
    /// the level below. Merging fours would come out cheaper and more accurate,
    /// but more accurate is exactly the breakage here: the exact test forgives
    /// a cell its turned-away-ness by the coverage radius of ITS probe grid,
    /// and that radius is its own at every rung and twice as long as at the
    /// rung below. A merged ball would carry the zero level's margin and would
    /// turn out stricter than the judge it must not contradict -- that is, it
    /// would reject what is visible, silently.
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

    /// The ball of a single cell: by the same nine probes that
    /// [`Self::on_screen`] takes, with a margin for the unchecked between them.
    ///
    /// The margin comes from [`covering_radius_deg`] -- the same measure by
    /// which [`faces_cap`] forgives the cell the turned-away-ness of all nine
    /// probes. And this is not a convenience but a necessity: the ball must not
    /// reject what the exact test admitted, and the exact test admits exactly
    /// this margin beyond its probes. Introduce a second measure here -- and
    /// they would diverge silently, with cells vanishing right at the limb.
    ///
    /// The arc, not the chord: the arc is longer, that is more generous, and
    /// generosity is what is needed here. This measure is not a strict upper
    /// bound -- its shortfalls are listed where it lives -- but those shortfalls
    /// are shared by both askers, and the answers cannot diverge because of
    /// them.
    ///
    /// On top of the margin goes the lift: a cell is drawn not on the common
    /// lift but on its own (`mesh::lift_m`), and for a coarsely divided one it
    /// is kilometres above the probe. The ball must cover what is drawn, not
    /// what was meant.
    fn ball(&self, meta: &Meta, level: u32, x: u32, y: u32) -> cull::Ball {
        let (points, seen) = self.probes(meta, level, x, y);
        // The span is taken from the same probes rather than counted anew:
        // `span_deg` asks the binding about the same nine points, and a second
        // pass over it would cost more than everything else here put together.
        let span = span_of(&seen);
        let lifted = mesh::lift_m(span / f64::from(segments_for(span)) * std::f64::consts::SQRT_2)
            - mesh::SURFACE_LIFT_M;
        let reach = covering_radius_deg(&seen).to_radians() * mesh::drawn_radius();
        cull::Ball::over(&points).widened(reach as f32 + geodesy::metres(lifted))
    }

    /// The nine probes of a cell: world points and their latitudes with
    /// longitudes.
    ///
    /// One place for everyone who needs them -- the exact test, the ball, and
    /// the one that measures the coverage radius. Counted separately they would
    /// diverge silently, and the inverse pass of the binding in them is the
    /// most expensive part.
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

    /// The cells of the level that entered the frame.
    ///
    /// What is walked is noted in `toll`: how many cells are checked and how
    /// many of them turn out visible. One addition per level rather than an
    /// increment per cell -- every last one of them is checked, and the number
    /// is known in advance.
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
                // A rough cut before the exact test: the ball answers "not
                // visible" in a dozen nanoseconds, while nine probes take
                // twelve microseconds. The balls may not be built (there has
                // been no description yet) -- then there is no cut, and only
                // the exact test is asked.
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

    /// Whether a cell is visible: its points are converted by the binding into
    /// world points and projected by the same matrix the frame is drawn with.
    /// The far side of the Earth filters itself out -- its points are turned
    /// away from the eye.
    ///
    /// What decides is not a point falling into the frame but an intersection:
    /// a cell can be larger than the whole screen, and then none of its corners
    /// is in the frame while only it is visible.
    ///
    /// Not four corners are taken but nine points with the side midpoints and
    /// the center, and this is not a margin for safety's sake. For a cell of the
    /// top rung of a global raster the corners turn out to be the poles: looking
    /// at the equator all four are turned away from the eye, and the cell that
    /// occupies the whole frame is declared invisible. The rung is then skipped
    /// silently -- and that is exactly the rung from which the image is supposed
    /// to appear whole.
    ///
    /// Nine points are not enough even for them: unchecked ground lies between
    /// the probes, and a cell can be four orders of magnitude larger than the
    /// visible piece of Earth. So a turned-away nine is still not an answer:
    /// after it they ask [`faces_cap`], which forgives the turned-away-ness by
    /// the coverage radius of the probe grid ([`covering_radius_deg`]). Only it
    /// is asked: a nine that said "visible" will not be changed by the margin,
    /// and only the cells that would otherwise be discarded pay for it.
    fn on_screen(&self, meta: &Meta, level: u32, x: u32, y: u32, look: &Look) -> bool {
        // The probes are taken from the shared place ([`Self::probes`]) -- the
        // same one the ball is built from: the inverse pass of the binding is
        // the most expensive thing here, and two copies of it would diverge
        // silently, and the ball would start rejecting what is admitted here.
        let (world, seen) = self.probes(meta, level, x, y);

        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        let mut faces = false;
        for point in world {
            faces |= faces_eye(point, look.eye);

            let clip = camera::project(&look.view_proj, point, look.eye);
            // A point behind the camera: one must not divide by such a w, and
            // the cell is knowingly close meanwhile -- we count it visible and
            // do not guess.
            //
            // Points do not land here at all, and not by chance: the view
            // matrix is a pure rotation, the look goes to the center of the
            // ellipsoid, whence `w = |eye| − ê·point`, and the point lies on
            // the lift, that is `w ≥ (H − lift)/a`. The camera floor is
            // derived from the lift (`camera::HEIGHT_RANGE_M`), so even at the
            // very floor there are a thousand `f32` quanta to spare. `w` does
            // not depend on the frame edge at all.
            //
            // The answer is still its own rather than via `faces`: the relation
            // between these quantities is declared nowhere, and
            // `faces || faces_eye(point, eye)` is identically equal to `faces`,
            // that is it would answer "not visible" because of a single
            // already-passed point.
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

/// Whether a surface point faces the eye. Counted on the sphere rather than
/// the ellipsoid: the flattening gives fractions of a percent, and the
/// question is crude -- whether the point is on this side of the horizon or on
/// that one.
fn faces_eye(point: World, eye: World) -> bool {
    let to_eye = [eye[0] - point[0], eye[1] - point[1], eye[2] - point[2]];
    point[0] * to_eye[0] + point[1] * to_eye[1] + point[2] * to_eye[2] > 0.0
}

/// How far from the nearest probe a cell's point may lie.
///
/// The nine probes stand in a 3×3 grid, and the farthest from them is the
/// middle of a quarter-cell: from it to each of the four surrounding probes is
/// half the diagonal of the quarter-cell. Hence the measure: half the diagonal
/// built on the longest steps of the grid.
///
/// Half of the longest step rather than a quarter of the cell's span. The two
/// are equal only while the middle lies in the middle, and with a grid of
/// uneven support points it does not lie there: on a row with longitudes 0, 1
/// and 20 a quarter of the span would call five degrees where the real nine
/// and a half stand.
///
/// In degrees, not the chord. For the top cell of a global raster both
/// "corners" are the poles, and a quarter of the chord between them would call
/// 45° where the truth is ninety: the fix would silently fail in exactly the
/// case it exists for.
///
/// This measure is not a strict upper bound, and all three of its shortfalls
/// must be known.
///
/// The degrees of [`arc_deg`] are geodetic while the angle must be
/// geocentric -- a near-polar step along a meridian is understated by
/// two-thirds of a percent because of it. A strip that went around the Earth
/// along a longitude away from the equator is probed by only two longitudes:
/// for the belt from −45° to 15° the measure gives 88°, while the far point
/// stands at ninety. And the main thing: the measure believes that nine probes
/// describe the cell. The interior of the grid is set by its nodes, while
/// three fractions along each axis are asked -- the bowing of the path between
/// them at a quarter and three quarters touches no probe and falls into no
/// step. It is lost from the crest by roughly eight tenths of the margin: for
/// a two-degree cell that is seventy kilometres. It can only be covered by
/// steps along the grid nodes rather than the probes; the support grids of
/// acquisitions bend by kilometres, and they are far from the threshold.
///
/// This measure is not the one of [`span_deg`], and the two cannot coincide:
/// that one answers how wide the cell is (to pick the warp-mesh density) and
/// measures by the ends; this one answers how densely it is probed, and
/// measures by the steps.
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

/// Whether a cell may touch the visible piece of Earth when it has been probed
/// with these probes.
///
/// [`faces_eye`] answers about a point and answers exactly; the same question
/// cannot be asked about a cell -- unchecked ground lies between the probes.
/// Here the turned-away-ness is forgiven by `spread` degrees: that far from the
/// nearest probe a cell's point may lie.
///
/// Counted without a single `acos`. The condition "a probe is closer than the
/// horizon plus the margin" is `γ < γ₀ + spread`, and `cos γ₀ = r/|eye|`,
/// whence the comparison of cosines unfolds into dot products and roots.
///
/// The horizon radius is taken from the lowest of the probes: the lower the
/// point, the farther is visible from it, and a cell one edge of which is
/// closer to the pole cannot be judged by its equatorial edge. The flattening
/// of the Earth is no trifle here -- twenty-one kilometres against the eight
/// hundred metres of the camera floor.
///
/// The margin grows with the cell, and on coarse rungs it shows: for a world
/// raster with a low camera the whole rung passes -- six cells instead of the
/// one visible. This is not the crudeness of the estimate but its design, and
/// a tile is its price: on such rungs there are only single cells while the
/// appetite ceiling counts in hundreds.
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
    // The eye can be closer to the center of the Earth than the probe: over
    // middle latitudes a low camera yields those same twenty-one kilometres to
    // an equatorial point. Then there is no horizon at all -- its sine is zero.
    let beyond = (dot(&eye, &eye) - lowest).max(0.0).sqrt();

    let (sin_spread, cos_spread) = spread_deg.to_radians().sin_cos();
    // The margin has rolled past the antipodal point of the sphere: beyond it
    // the cosine grows again, and answering by comparison with it is no longer
    // possible -- and there is nothing to answer, everything is visible.
    if beyond * cos_spread + radius * sin_spread < 0.0 {
        return true;
    }
    let reach = radius * cos_spread - beyond * sin_spread;
    points
        .iter()
        .zip(&lengths)
        .any(|(point, length)| dot(point, &eye) > length.sqrt() * reach)
}

/// A cell a carrier was found for -- and everything to draw it with.
///
/// Counted separately from the drawing itself because this is asked more often
/// than drawing: the store access prolongs the tiles' life in the budget, and
/// it also answers whether what is drawn has changed (see [`Overlay::built`]).
pub struct Piece {
    /// The cell of the chosen rung, as it was asked for.
    cell: tiles::Addr,
    /// What covers it: its own tile or an ancestor if its own is not there yet.
    carrier: tiles::Addr,
    bind: BindGroupId,
    /// Its place in the raster's pixels -- the nodes are counted by it.
    rect: [f64; 4],
    /// Its piece of the carrier's texture.
    uv: [f32; 4],
    /// The cell's own tile is on its way right now: it is covered by an
    /// ancestor, and what is seen is about to get sharper. It is exactly this
    /// place on the globe that marks the load in progress (see
    /// `gpu::OverlayVertex::ripple`).
    coming: bool,
}

/// A cell as the fingerprint remembers it: the cell itself, what covers it,
/// what bind group that texture has, and whether a replacement is on its way.
///
/// As numbers and a flag rather than the [`Piece`] itself: the fingerprint is
/// compared every frame, and exactly what the vertices depend on must be
/// compared in it. The in-flight flag enters here because it is in the vertex
/// and baked in; it changes along with the tile's arrival, that is when the
/// vertices are reassembled anyway. But the ripple strength is not here and
/// must not be -- it is a layer's quantity, not a cell's, and put into the
/// vertex it would call for reassembling the whole mesh on every movement of
/// bytes (see `gpu::CameraUniform::ripple`).
pub type Drawn = (tiles::Addr, tiles::Addr, u64, bool);

impl Piece {
    /// Its trace in the fingerprint of what is drawn.
    pub fn drawn(&self) -> Drawn {
        (self.cell, self.carrier, self.bind.id(), self.coming)
    }

    /// The cell's own tile is on its way, and it is covered by an ancestor
    /// meanwhile.
    pub fn coming(&self) -> bool {
        self.coming
    }
}

/// Visible cells a carrier was found for.
///
/// The cells that come are the same ones that went into the tile request (see
/// [`Overlay::wanted`]): drawing something other than what was asked for means
/// either asking for the invisible or not showing what was fetched.
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
        // A cell covered by its own tile does not ripple even when it is
        // awaited: the arrival of its own tile clears the waiting, and there is
        // no need to ask it about the flight.
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

/// Patches of the overlay: one per cell with a carrier. Nodes and indices go
/// into shared buffers, draw calls are index ranges by the carrier.
///
/// The nodes are taken from the raster's cache and put back into it: a node's
/// place on Earth depends neither on the camera nor on the tile under it, and
/// it costs more than everything else in the patch put together (see
/// [`Nodes`]). Exactly the drawn cells stay in the cache -- otherwise it would
/// grow on everything that ever entered the frame.
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
        // The cell's first node is everything that distinguishes its drawing
        // from a neighbor's of the same density: the indices are shared (see
        // `gpu::OverlayBatch::block`).
        let base = vertices.len() as u32;
        // By the offset, not the strength: the strength is a layer's quantity
        // and rides in the frame's uniform (see [`Drawn`]).
        spread(nodes, piece.uv, *opacity, ripple_of(piece.cell, piece.coming), vertices);
        draws.push(Patch { bind: piece.bind.clone(), segments: nodes.segments, base });
    }
}

/// How many distinct phase offsets are dealt to cells. Sixteen -- enough for
/// neighbors to diverge noticeably, and not so many that the ripple breaks
/// into noise: a cell in the frame rarely has more than a dozen neighbors.
const RIPPLE_OFFSETS: u32 = 16;

/// What the cell answers the shader about the ripple: zero -- it does not
/// ripple, otherwise its own phase offset in (0, 1].
///
/// The offset is derived from the address rather than dealt in traversal
/// order: the order of cells in the frame changes with every camera movement,
/// and the wave would jump from place to place over a still picture. Mutually
/// prime multipliers -- so that neighbors in a row and in a column do not land
/// on the same offset.
///
/// Zero is excluded on purpose: it is how the shader tells a non-rippling
/// cell, and a cell with an offset exactly at the start of the period would be
/// indistinguishable from it.
fn ripple_of(cell: tiles::Addr, coming: bool) -> f32 {
    if !coming {
        return 0.0;
    }
    let (_, x, y) = cell;
    let slot = (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(11))) % RIPPLE_OFFSETS;
    (slot as f32 + 0.5) / RIPPLE_OFFSETS as f32
}

/// The warp-mesh nodes of a single cell -- everything in the patch that depends
/// neither on the camera nor on the tile under it.
///
/// As halves of a point rather than a ready vertex: `uv` changes when the tile
/// moves to another texture, the opacity -- when the layer's slider is moved,
/// while the node's place on Earth changes with neither. The difference in cost
/// is the point of this split: a node's place is the binding plus two
/// trigonometries (`geodesy::world`), that is a hundred and fifty nanoseconds,
/// while `uv` is a multiplication.
struct Nodes {
    /// Segments per side: both the fractions inside the cell and the indices
    /// are derived from them.
    segments: u32,
    /// The nodes row by row, `(segments + 1)²` of them, each as halves of a
    /// world point.
    points: Vec<(geodesy::World, geodesy::World)>,
}

/// The warp-mesh nodes of a cell: each one's place is the exact projection of
/// the binding.
///
/// The density is derived from how much arc the cell occupies rather than
/// assigned as a number: for a granule that is fractions of a degree, for a
/// global raster -- all 360°, and one and the same division cannot serve both
/// (see [`MAX_PATCH_SEGMENTS`]). What the division does not reach is made up
/// by the height -- the rule and its cost are described at `mesh::lift_m`. The
/// lift is safe for neighbors: the outlines and the grid are drawn after the
/// overlays and write no depth (see `gpu::render`).
fn nodes_of(frame: &Frame, meta: &Meta, cell: [f64; 4]) -> Nodes {
    let span = span_deg(frame, meta, cell);
    let segments = segments_for(span);
    // The sag is measured by the quad's diagonal rather than its edge: the quad
    // is cut into two triangles, and the middle of the hypotenuse dives deepest
    // under the surface -- it is √2 times farther than the middle of a side,
    // while the sag grows as the square of the distance. Lifted along the
    // edges, the triangles of the top rung of a global raster would still dive
    // under the ellipsoid, and the image would be seen in patches right at the
    // nodes.
    let height_m = mesh::lift_m(span / f64::from(segments) * std::f64::consts::SQRT_2);

    // A node is computed once: neighboring quads share them, and the
    // projection is the most expensive part of a node.
    let side = segments + 1;
    let mut points = Vec::with_capacity((side * side) as usize);
    let at = |tx: f64, ty: f64| {
        let fx = (cell[0] + (cell[2] - cell[0]) * tx) / f64::from(meta.width);
        let fy = (cell[1] + (cell[3] - cell[1]) * ty) / f64::from(meta.height);
        frame.geodetic(fx, fy)
    };
    let share = |step: u32| f64::from(step) / f64::from(segments);

    // For an axial frame a grid row is a parallel and a column is a meridian:
    // the latitude does not depend on the fraction across the width, the
    // longitude -- on the fraction along the height. Then both trigonometries
    // are computed once per row and once per column, and a node comes out as
    // six multiplications (see [`geodesy::Parallel`]). For a cell of 35 nodes
    // per side that is seventy expansions instead of a thousand and a quarter.
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

/// Ready nodes -- into vertices with their own piece of the carrier's texture.
///
/// The opacity rides in the vertex rather than in a uniform: all overlay
/// patches lie in one buffer and are drawn by one pipeline, differing only in
/// the index range and the carrier's bind group -- the layer has no uniform of
/// its own here and there is nothing to start one for.
///
/// A node goes into the buffer once: the triangles refer to it by indices, and
/// the indices are shared by all cells of the same density and lie separately
/// (`gpu::OverlayBatch::block`). A cell has `(s+1)²` nodes, while a triangle
/// list of vertices would be `6s²` -- more than four times over for a granule
/// (`s = 8`) and more than five times over for anything larger.
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
    // The column fractions are computed once per patch rather than per node:
    // there are (s+1) of them and (s+1)² nodes, and the division is the most
    // expensive thing left in this pass.
    let us: Vec<f32> = (0..side).map(|col| uv[0] + (uv[2] - uv[0]) * (col as f32 / across)).collect();
    vertices.reserve(nodes.points.len());
    for (row, line) in nodes.points.chunks(side as usize).enumerate() {
        let v = uv[1] + (uv[3] - uv[1]) * (row as f32 / across);
        for (&u, &(position, position_low)) in us.iter().zip(line) {
            vertices.push(OverlayVertex { position, position_low, uv: [u, v], alpha, ripple });
        }
    }
}

/// Into how many parts to split the side of a cell of such a span -- so that
/// the quad's sag fits under the common lift.
fn segments_for(span_deg: f64) -> u32 {
    ((span_deg / mesh::max_quad_side_deg()).ceil() as u32)
        .clamp(PATCH_SEGMENTS, MAX_PATCH_SEGMENTS)
}

/// The same span as [`span_deg`], but from already computed probes.
///
/// The probe order is the same as in [`Overlay::probes`]: the X fraction is
/// the minor one, that is row `r` occupies slots `3r..3r+3`.
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

/// How much arc the cell occupies -- by its longest row.
///
/// By the rows rather than the diagonal: the cell is split along them, and it
/// has to be divided by whichever of them is longer.
///
/// Not only the edges are measured but the middle as well, and this is not a
/// margin for safety's sake. A binding can be anything, and for a global raster
/// the edges of the top rung degenerate into a point at the pole: all its
/// length is in the middle row, while along the edges it comes out half as
/// narrow as it is. A cell lifted by such a measure dives under the body of
/// the Earth for kilometres, and an image covering the whole Earth is again
/// seen in patches right at the grid nodes -- exactly what the density is
/// computed for.
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
