//! A cheap refusal for a cell: the ball against the frame.
//!
//! The exact answer to "is the cell visible" costs nine inverse projections — about six
//! hundred transcendental calls per cell, and almost all of them in the inverse binding.
//! A detailed raster has close to two thousand cells per level, and they are asked on
//! every frame of camera motion, so this count is the most expensive thing the module
//! does per frame.
//!
//! A coarse cull stands in front of it here: the cell is covered by a ball, and the ball
//! is tested against the four side planes of the frame and against the horizon. Thirty-odd
//! additions and multiplications, not a single transcendental call. The one that says
//! "not visible" is believed and the exact test is not called at all; the one that says
//! "visible" is not believed, and the verdict is delivered by the same old exact test.
//!
//! Hence the requirement on the ball: it must **cover** the cell. A ball wider than needed
//! costs an extra call of the exact test; a ball narrower — a silently lost tile.

use glam::Vec3;

use super::camera::Mat4;
use super::geodesy::{self, World};

/// A ball covering the cell. In the same shares of the semi-major axis as the whole world
/// (`geodesy::world`), and in `f32`: the test is coarse, and the ball table lies over the
/// whole raster, where half its size costs more than the last digit.
#[derive(Clone, Copy, Debug)]
pub struct Ball {
    pub centre: [f32; 3],
    pub radius: f32,
}

impl Ball {
    /// A ball over a set of points: the middle of the bounding box and the largest distance
    /// to a point.
    ///
    /// The middle of the bounding box, not the mean: the mean shifts toward the clump of
    /// points, and for a cell with a degenerate edge (the corners of the top step of the
    /// global raster — both poles) the radius grows from it instead of shrinking.
    pub fn over(points: &[World]) -> Self {
        let mut low = [f32::MAX; 3];
        let mut high = [f32::MIN; 3];
        for point in points {
            for axis in 0..3 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
        let centre = [
            (low[0] + high[0]) * 0.5,
            (low[1] + high[1]) * 0.5,
            (low[2] + high[2]) * 0.5,
        ];
        let radius = points.iter().map(|point| distance(centre, *point)).fold(0.0, f32::max);
        Self { centre, radius }
    }

    /// Give the ball a margin — for what the probes did not see.
    pub fn widened(self, by: f32) -> Self {
        Self { radius: self.radius + by, ..self }
    }

}

fn distance(from: [f32; 3], to: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// What refuses a cell: four side planes of the frame and the horizon.
///
/// There are no near and far planes here. The far one is derived from the size of the Earth
/// and cuts nothing, and the near one is needed even less: the sum of the left and the
/// right gives the doubled view forward, that is, the four side planes themselves cull
/// everything behind the back. Fewer planes — a more cautious answer, and caution is what
/// is needed here.
pub struct Frame {
    /// Normals directed into the frame. They have no free term: the point is subtracted
    /// from the eye before the matrix (`camera::project`), and the view matrix is a pure
    /// rotation, whence its fourth row ends in zeros.
    inward: [Vec3; 4],
    eye: Vec3,
    /// Where the eye looks from the center of the Earth, and at what height above this
    /// direction the horizon lies.
    towards: Vec3,
    horizon: f32,
}

impl Frame {
    /// Planes from the frame matrix.
    ///
    /// The layout is column-major (glam), multiplication `M·v`, so the rows are taken with
    /// `Mat4::row`. The left plane is the sum of the fourth row with the first, the right
    /// one is their difference, and likewise vertically; the sign follows from `|x| ≤ w`
    /// inside the frame.
    ///
    /// This rule lives here and in `camera::project`, and the two must not diverge: the
    /// check `a_point_inside_the_frame_clears_every_plane` asks both about a single point.
    pub fn new(view_proj: &Mat4, eye: World) -> Self {
        let (row0, row1, row3) = (view_proj.row(0), view_proj.row(1), view_proj.row(3));
        let inward = [row3 + row0, row3 - row0, row3 + row1, row3 - row1]
            .map(|plane| plane.truncate().normalize());
        let eye = Vec3::from(eye);
        let towards = eye.normalize();
        // The horizon is taken by the polar radius — the smallest of the possible ones.
        // It gives the lowest horizon, that is, the widest half-space and the most cautious
        // answer. The cell's local radius would be more precise and could turn out larger —
        // that is, cut off what is visible.
        // The polar radius is in the same shares as the world: the flattening is exactly
        // how much closer to the center the pole is than the equator (`geodesy::world`).
        let polar = (1.0 - geodesy::FLATTENING) as f32;
        Self { inward, eye, towards, horizon: polar * polar / eye.length() }
    }

    /// Whether this cell can be visible.
    ///
    /// "Can" — because the answer is one-sided: `false` means "definitely not visible",
    /// `true` — "ask the exact test next".
    pub fn admits(&self, ball: &Ball) -> bool {
        let centre = Vec3::from(ball.centre);
        let from_eye = centre - self.eye;
        // Behind the horizon the cell goes entirely only if its ball lies entirely on the
        // far side. An eye inside the ball does not fall through here: then
        // `centre·ê + r ≥ |eye| ≥ polar radius ≥ horizon`, and the answer comes out
        // "visible" by itself, without a special branch.
        if centre.dot(self.towards) + ball.radius < self.horizon {
            return false;
        }
        self.inward.iter().all(|inward| inward.dot(from_eye) + ball.radius >= 0.0)
    }
}
