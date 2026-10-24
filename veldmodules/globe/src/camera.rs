//! Camera over the Earth and the matrices derived from it.
//!
//! The camera is a frame above the surface: the unit vector `at` shows the point
//! it hangs over, the unit vector `up` — what is at the top of the screen, and
//! the height says how far it is pulled back. It looks at the center of the
//! ellipsoid.
//!
//! ## Why a frame and not a pair of angles
//!
//! Everything else in the file follows from this. Latitude and longitude are
//! coordinates with a pole: meridians converge there, "east" stops meaning
//! anything, and a gesture in these coordinates has to be either scaled by
//! 1/cos(latitude) or stopped by a wall short of the top. The frame has no
//! pole: a gesture is a rotation about the camera's own axes, the arc comes out
//! exactly the one asked for at any latitude, and the camera crosses the pole
//! without noticing it. Latitude and longitude are derived from `at` when they
//! are asked about — and never the other way around.
//!
//! The Earth itself stays still: it has its own coordinate frame, and one would
//! have to rotate it together with it — and then "what point is under the
//! cursor" would stop being a question for a single matrix.
//!
//! ## Everything is measured by what is visible
//!
//! Height by itself means nothing: both the gesture and the aim at a place speak
//! of an angle — how much arc fits in the frame. [`Camera::visible_deg`] computes
//! that quantity, and inverted it answers the question "from what height this
//! much is visible" ([`height_for`]). Hence the main property of the controls:
//! dragging across the full height of the area turns by exactly what is visible
//! in it — at any height.
//!
//! ## The quaternion here is exactly one, and only where it beats the frame
//!
//! The camera stores a frame, not a quaternion: consumers (the view matrix, the
//! ray under the cursor) need three axes, and a quaternion would have to be
//! unfolded into them every frame. But a flight has a different task — to carry
//! the frame between two orientations — and here the frame loses: interpolating
//! a pair of unit vectors componentwise means walking them along a chord and
//! fixing up with Gram–Schmidt, that is, taking a different path and pretending
//! it is the requested one. Therefore [`Flight`] keeps its ends as quaternions
//! and moves between them with slerp — the single operation for which they are
//! introduced here at all.

use glam::{DMat3, DMat4, DQuat, DVec3, Vec3, Vec4};

/// A 4×4 matrix in the WGSL layout: by columns. It is exported from here
/// because the uniform layout is this file's contract with the shader.
pub use glam::Mat4;

use crate::module::geodesy::{self, Geodetic};
use crate::module::mesh;

/// Vertical field of view. Not only the projection is measured by it: the
/// gesture and the aim use it to compute how much arc fits in the frame — so
/// there is a single one for everyone.
const FOV_Y_DEG: f64 = 50.0;

// How much one wheel click zooms in — the same step as the preview canvas,
// from a single file (see `wheel.rs`). What it multiplies differs: scale for
// the canvas, height for the globe. At the surface these are the same, but at
// a distance the globe grows more slowly — the angle at which it is seen is
// not proportional to height.
use super::wheel::ZOOM_PER_STEP;

/// Height above the ellipsoid: from the view where the image is read pixel for
/// pixel to the view where the Earth is a small ball.
///
/// The lower bound is about detail, not safety, and it is derived from
/// [`FOV_Y_DEG`]: from height `h` **above the drawn surface** `0.93·h` meters of
/// surface fit vertically in the frame, that is, with a thousand-row window —
/// `h/1070` meters per pixel.
///
/// The lower bound is counted off the lift, not assigned by itself: descending
/// right to the lift means passing through the drawn surface. The tenfold margin
/// is a choice, not a derived quantity: nothing breaks at fivefold either, the
/// lift just occupies a fifth of the height there, and there is nothing to
/// examine from such a mark.
///
/// The lift introduces no distortion: everything the camera measures above the
/// visible measures before it ([`Camera::visible_deg`]). Coordinate accuracy is
/// not a limit here — it comes from the distance to the eye, and at the floor it
/// is tens of microns (`geodesy::parts`).
///
/// Eight hundred meters is seven hundred above the drawn surface, that is,
/// seven tenths of a meter per pixel with a thousand-row window: finer than any
/// image that arrives here, and enough to trace a field boundary by hand.
const HEIGHT_RANGE_M: (f64, f64) = (mesh::SURFACE_LIFT_M * 10.0, 80_000_000.0);

/// How many times more ground a pixel under the cursor may cover than it would
/// cover at nadir, before the hold starts to weaken.
///
/// Twice is a choice, not a derived quantity, and it is chosen by the width of
/// the band where the hold weakens: the cosine falls steeply at the rim, and at
/// fourfold it shrinks to a dozen cursor pixels — the rim still jerks the view,
/// only over a shorter path. The measure is the grazing angle, not the distance
/// from the center of the frame: holding stops where the surface leaves the ray,
/// that is, on the approach to the silhouette. And how far the silhouette is
/// depends not on height alone: at the vertical edge of the frame it shows only
/// at a distance, while in the corner of a 16:9 panel — three times lower, and
/// the hold weakens there as well. This is not a caveat but the same rule: the
/// corner of the panel looks at the ground much more obliquely.
const GRAZE_LIMIT: f64 = 2.0;

/// Margin around the aimed point: an outline touching the frame edge reads as
/// cut off.
const FRAME_MARGIN: f64 = 1.3;

/// How long the flight to the aimed point lasts, seconds.
///
/// One number for any distance: a flight explains where the view went, it does
/// not measure out the path. Stretched in proportion to the arc, it would last
/// half a minute for an aim from the other hemisphere — and that would be
/// waited for, not watched.
const FLIGHT_S: f64 = 0.6;

/// A flight to the aimed point: the ends and the elapsed time.
///
/// Ends, not "goal and velocity": the first has an end, the second only implies
/// one, and the camera would approach it forever.
#[derive(Clone, Copy, PartialEq)]
struct Flight {
    from: DQuat,
    to: DQuat,
    from_height_m: f64,
    to_height_m: f64,
    passed_s: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Camera {
    /// Where the camera looks — a unit normal to the ellipsoid at the point
    /// under the camera. Latitude and longitude are derived from it (see
    /// `geodesy::angles`), not stored: a pair of angles has a pole, a direction
    /// does not.
    at: DVec3,
    /// What is at the top of the screen — a unit vector orthogonal to `at`.
    /// Stored, not derived from the world axis: derived, it is undefined at the
    /// pole, and the gesture would hit a wall exactly there.
    up: DVec3,
    /// Height is measured from the ellipsoid surface, so "100 km" means 100 km
    /// both at the equator and at the pole.
    height_m: f64,
    /// The flight in progress. `None` — the camera stands where it was left.
    flight: Option<Flight>,
}

impl Default for Camera {
    /// The initial view — all of Eurasia: what the application is opened for.
    fn default() -> Self {
        let at = geodesy::unit(45.0, 60.0);
        Self { at, up: frame_up(at, DVec3::Z), height_m: 14_000_000.0, flight: None }
    }
}

impl Camera {
    /// Dragging: the Earth moves with the cursor. Hence the signs — the camera
    /// turns to meet the gesture, not along with it.
    ///
    /// A share of the area is converted to degrees by what is visible in it: at
    /// a distance it is a hundred and fifty degrees and more, right at the
    /// surface — fractions of a degree. A constant rate would suit exactly one
    /// height, and at all the others the gesture would be either sluggish or
    /// slipping out of hand.
    ///
    /// There is no latitude scaling here and there cannot be: a horizontal
    /// gesture is a rotation about the frame's own vertical, not an increment of
    /// longitude, and its arc is the same at the equator and near the top.
    ///
    /// The camera crosses the pole and ends up beyond it with north pointing
    /// down. This is not a failure but the same as seen from an airplane window:
    /// having crossed the top, one looks backward. The gesture does not take it
    /// upon itself to finish turning the frame — that would spin the world half
    /// a turn before one's eyes; north is brought back by the same dragging back
    /// or by aiming at an image (see [`Camera::focus`]).
    ///
    /// A gesture and its exact inverse cancel each other — and a diagonal one
    /// too, because there is a single rotation here. Split into yaw and pitch,
    /// it would be cancelled only in the limit of a small step: two rotations of
    /// finite size about different axes do not compose into the one asked for,
    /// and at full distance a degree of miss and a degree of roll accumulate.
    ///
    /// Going around in a circle, however, returns the view turned by the area of
    /// the traversed loop — that is a property of the sphere, and it stays: no
    /// choice of axes removes it.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        // A gesture takes over the control: continuing the previous aim while it
        // is moved by hand means pulling the camera in two directions at once.
        self.flight = None;

        // Across the full height of the area twice as much is visible as from
        // the center to the edge.
        let span = 2.0 * self.visible_deg();
        // The rotation axis is composed of the frame axes, and the angle is its
        // length: for a horizontal gesture it is the frame's vertical, for a
        // vertical one — the horizontal, for a diagonal — whatever lies between.
        let turn = (self.up * -f64::from(dx) + self.side() * -f64::from(dy)) * span;
        let angle_deg = turn.length();
        if angle_deg <= 0.0 {
            return;
        }

        let spin = DQuat::from_axis_angle(turn / angle_deg, angle_deg.to_radians());
        self.at = spin * self.at;
        self.up = spin * self.up;
        self.settle();
    }

    /// Zoom in, keeping the ground under a frame spot where it was.
    ///
    /// The held point is given as an offset from the center of the frame in
    /// shares of the **height** of the area, both axes; zeros — zoom toward the
    /// center. In shares of height, not of each axis's own side — for the same
    /// reason as in [`Camera::orbit`]: the field of view is set vertically, and
    /// the proportions of the area do not enter at all.
    ///
    /// Zooming is multiplying the height, and the hold is a final turn of the
    /// frame: the point under the cursor stands off the point under the camera
    /// by [`Camera::arc_at`], and from the new height the same frame spot looks
    /// elsewhere already. The turn plays back the difference of the two arcs.
    ///
    /// It is computed in two passes, and the second is not a luxury. The arc is
    /// spherical, while what it turns is a geodetic normal; height is measured
    /// along the normal, and the frame looks at the center. Both misses together
    /// drift the frame by a pixel per click — and multiply on the way down,
    /// because each next click enlarges what has already moved. Therefore the
    /// place is asked again, precisely now, and the frame turns by the amount it
    /// diverged from the promised one. The second pass also removes the sideways
    /// drift that zooming toward the center has, where there is no first pass.
    pub fn zoom_at(&mut self, steps: f32, hold_x: f32, hold_y: f32) {
        if !(steps.is_finite() && hold_x.is_finite() && hold_y.is_finite()) {
            return;
        }
        // A gesture takes over the control: continuing the previous aim while it
        // is moved by hand means pulling the camera in two directions at once.
        self.flight = None;

        // Height shares are counted top-down, while the frame's Y axis points up.
        let (right, above) = frame_offset(f64::from(hold_x), -f64::from(hold_y));

        let height_m = (self.height_m * ZOOM_PER_STEP.powf(f64::from(steps)))
            .clamp(HEIGHT_RANGE_M.0, HEIGHT_RANGE_M.1);
        // Right at the silhouette rim the hold weakens — not for the sake of the
        // arithmetic, but because there is nothing to hold there: the surface
        // leaves the ray, a single frame pixel covers kilometers (see
        // `geodesy::intersect_at`), and a jitter of the cursor by a pixel would
        // put a completely different place under it. It weakens by keeping the
        // point closer to the center: the same frame spot, only a recognizable
        // one. Otherwise at the rim a click would spin the globe by a sixth of
        // the frame, and a pixel further — by nothing at all.
        //
        // The grip is asked at both heights — the current one and the one being
        // traveled to — and the smaller is taken. Pulling back, the camera
        // shrinks the disk, and the held place ends up beyond the silhouette: it
        // can no longer be held there, and this must be known before the turn,
        // not after.
        //
        // The pair is symmetric, and that is the whole point: a click back has
        // the same two heights, hence the same grip, hence it cancels the click
        // forward. Were it to peek at the click forward along the way, the two
        // directions would come out different, and the pair would stop being a
        // pair.
        let alpha_seen = right.hypot(above).atan();
        let share = self.grip(alpha_seen).min(Self { height_m, ..*self }.grip(alpha_seen));
        let (right, above) = (right * share, above * share);
        let alpha = right.hypot(above).atan();
        // The place is taken before the height change and taken precisely — the
        // same way as at a click (see [`Camera::probe`]): against the ellipsoid,
        // with the lift, in f64.
        let held = geodesy::intersect_at(
            geodesy::world(self.geodetic()),
            self.look(right, above),
            mesh::SURFACE_LIFT_M,
        );
        let before = self.arc_at(alpha);

        // Hitting the clamp is a zoom that did not happen: the height is the
        // same, and so is the place in the frame, so there is nothing to do.
        let moved = height_m != self.height_m;
        self.height_m = height_m;

        // There is no ground under the cursor only beyond the very edge of the
        // frame: the grip pulls the held point inside the disk before the ray
        // can miss. The live reason to exit here is a zoom that did not happen;
        // a miss is caught along the way, so as not to guess.
        let Some(held) = held.filter(|_| moved) else { return };

        if let (Some(before), Some(after)) = (before, self.arc_at(alpha)) {
            let (side, up, _) = self.basis();
            self.swing(side * right + up * above, before - after);
        }

        // The screen mismatch goes into the arc, with the same lag, but already
        // from the new height. The arc under this angle cannot be missing: the
        // grip pulled the point inside the disk earlier, and it asked both
        // heights before the turn. The branch stands so as not to guess, not
        // because something hides behind it.
        let Some(slope) = self.slope_at(alpha) else { return };
        // Across the frame radius the gain is different, and this is not a
        // subtlety. The final turn goes about an axis lying in the plane of the
        // arc, so the place walks around it in a circle and the azimuth changes
        // by `δ·ctg ψ`, while the screen mismatch across is the azimuth times
        // the angle. Hence `tg ψ / α`, not `dψ/dα` and not the bare ratio of arc
        // to angle: at the center of the frame all three coincide, and away from
        // it they diverge by a quarter and more — and applying the radial gain
        // across leaves up to a pixel of mismatch where the correct one leaves
        // ten-thousandths.
        let across_slope = match self.arc_at(alpha) {
            Some(arc) if alpha > 0.0 => arc.tan() / alpha,
            // The exact center of the frame is held: there is no arc, and all the
            // gains coincide there.
            _ => slope,
        };
        let length = right.hypot(above);
        // There are two passes, and this number is measured, not chosen: on the
        // worst of the positions tried (`a_zoom_holds_whatever_it_took_hold_of`)
        // one leaves a hundredth and a half of a pixel, two — ten-thousandth and
        // a half, three — two millionths. One is not enough by itself, but on
        // the way down: every following click multiplies the mismatch of the
        // previous one, and there are forty-four clicks from the whole-Earth
        // view to the floor.
        for _ in 0..2 {
            // Where the place actually ended up. It is asked the same way the
            // frame is drawn — by the frame axes — and therefore catches any
            // miss at once, without sorting out whose it is.
            let Some((was_right, was_above)) = self.seen_at(held) else { return };
            let now = was_right.hypot(was_above);
            // Where the place moved: along the frame radius and across it.
            // Separately, because the two have different gains.
            let (along, across) = match length > 0.0 && now > 0.0 {
                true => (
                    now.atan() * (right * was_right + above * was_above) / (length * now) - alpha,
                    now.atan() * (right * was_above - above * was_right) / (length * now),
                ),
                // The center of the frame is held: there is no radius, and all
                // that is left is radial, and the gains coincide there anyway.
                false => (now.atan() - alpha, 0.0),
            };

            let (side, up, _) = self.basis();
            let (toward, sideways) = match length > 0.0 {
                true => (
                    (side * right + up * above) / length,
                    (up * right - side * above) / length,
                ),
                // The center of the frame is held: there is no radius, and the
                // direction is set by the mismatch itself. If there is none
                // either, there is nothing to turn, and `swing` zeroes out a
                // zero axis.
                false => match now > 0.0 {
                    true => ((side * was_right + up * was_above) / now, up),
                    false => (side, up),
                },
            };
            self.swing(toward, along * slope);
            self.swing(sideways, across * across_slope);
        }
    }

    /// Turn the frame by `angle` radians toward `toward` — a vector in world
    /// axes, of which only the part across the view counts.
    ///
    /// The rotation axis is composed of the normal under the camera and this
    /// vector, and for a point exactly in the center of the frame there is no
    /// such vector at all. This is not an edge case but an everyday one — the
    /// cursor in the middle of the area — and it cannot be kept silent about:
    /// `normalize` of a zero vector gives NaN, and nothing fixes a frame made of
    /// NaN.
    fn swing(&mut self, toward: DVec3, angle: f64) {
        let axis = self.at.cross(toward);
        let length = axis.length();
        if length <= 0.0 || angle == 0.0 || !angle.is_finite() {
            return;
        }
        let spin = DQuat::from_axis_angle(axis / length, angle);
        self.at = spin * self.at;
        self.up = spin * self.up;
        self.settle();
    }

    /// Aim the camera at a place so that a circle of the given angular radius
    /// is visible around it.
    ///
    /// The aim puts north up: one is brought to an image to look it over, and
    /// the roll accumulated by gestures has nothing to do with that. Right at
    /// the pole "north up" is undefined — there the previous top is taken, and
    /// this contradicts nothing: any turn of the frame above the top is equally
    /// good.
    ///
    /// The camera does not jump, it travels: a leap across half the globe does
    /// not read as movement — the view simply becomes a different one, and one
    /// has to reason out where the previous one went. [`Camera::advance`] drives
    /// it on the frame tick.
    pub fn focus(&mut self, lat_deg: f64, lon_deg: f64, radius_deg: f64) {
        let at = geodesy::unit(lat_deg, lon_deg);
        let up = frame_up(at, self.up);
        let height_m =
            height_for(radius_deg * FRAME_MARGIN).clamp(HEIGHT_RANGE_M.0, HEIGHT_RANGE_M.1);

        // The same goal as the one already in progress is no reason to start
        // over: were the aim to arrive every frame, the flight would return to
        // the start every time and the camera would crawl without ever arriving.
        // From outside this looks not like a hang but like "almost moving", and
        // so it costs more than an ordinary breakage.
        let to = orientation(at, up);
        if self.flight.is_some_and(|flight| flight.to == to && flight.to_height_m == height_m) {
            return;
        }

        self.flight = Some(Flight {
            from: orientation(self.at, self.up),
            to,
            from_height_m: self.height_m,
            to_height_m: height_m,
            passed_s: 0.0,
        });
    }

    /// Advance the flight in progress by a frame. `false` — there is nothing to
    /// drive.
    ///
    /// The answer is needed not for redrawing — the frame is compared against
    /// the drawn one as a whole, and a moved camera is visible there by itself —
    /// but for the tiles: the level and the set of visible overlay cells are
    /// computed from the camera, and while it travels they have to be asked for
    /// again (see `module::on_ui_event`).
    pub fn advance(&mut self, dt_s: f32) -> bool {
        let Some(mut flight) = self.flight else { return false };

        flight.passed_s += f64::from(dt_s).max(0.0);
        let part = (flight.passed_s / FLIGHT_S).clamp(0.0, 1.0);
        // It starts and stops smoothly: at a constant speed both ends of the
        // flight look like a jerk, and especially the end — the view freezes
        // instantly.
        let eased = part * part * (3.0 - 2.0 * part);

        let spin = flight.from.slerp(flight.to, eased);
        self.at = spin * DVec3::Z;
        self.up = spin * DVec3::Y;
        // The height is driven in shares, not in meters: zooming is multiplying
        // (see [`Camera::zoom_at`]), and with a uniform step the whole flight
        // would be eaten by the first frame while the last kilometers crawled.
        let times = flight.to_height_m / flight.from_height_m;
        self.height_m = flight.from_height_m * times.powf(eased);

        match part >= 1.0 {
            // The end is exactly the camera that was asked for, not what is left
            // over from rounding on the last step.
            true => {
                self.at = flight.to * DVec3::Z;
                self.up = flight.to * DVec3::Y;
                self.height_m = flight.to_height_m;
                self.flight = None;
            }
            false => self.flight = Some(flight),
        }
        true
    }

    /// Where the camera hangs, in the same coordinates as all the rest of the
    /// geometry. Derived, not stored (see the file header).
    fn geodetic(&self) -> Geodetic {
        let (lat_deg, lon_deg) = geodesy::angles(self.at);
        Geodetic { lat_deg, lon_deg, height_m: self.height_m }
    }

    /// The screen axis to the right — also the axis of the vertical gesture.
    fn side(&self) -> DVec3 {
        self.up.cross(self.at).normalize()
    }

    /// Restore orthonormality to the frame: rotations lose it drop by drop, and
    /// both the view matrix and the ray under the cursor are derived from it.
    fn settle(&mut self) {
        self.at = self.at.normalize();
        let up = self.up.reject_from_normalized(self.at);
        // Zero would mean that "up" has met the view direction — no gesture
        // makes such a rotation, but the frame must not silently stay
        // degenerate: all the geometry of the frame is derived from it.
        if up.length_squared() > 1e-12 {
            self.up = up.normalize();
        }
    }

    /// The angular radius of the visible: how many degrees of arc separate the
    /// point under the camera from the frame edge along the vertical. A special
    /// case of [`Camera::arc_at`], where the derivation is written out.
    ///
    /// The flattening of the ellipsoid is neglected here: at the pace of a
    /// gesture its third of a percent is invisible, while an honest computation
    /// would require knowing which way the gesture leads. Zooming under the
    /// cursor needs more than that — it removes the miss by a final turn at the
    /// place (see [`Camera::zoom_at`]).
    fn visible_deg(&self) -> f64 {
        match self.arc_at((FOV_Y_DEG * 0.5).to_radians()) {
            Some(arc) => arc.to_degrees(),
            // The frame edge misses the Earth: then it is visible out to the
            // horizon, and there is nothing beyond it.
            None => (mesh::drawn_radius() / self.radius()).acos().to_degrees(),
        }
    }

    /// The cosine of the angle at which the ray meets the surface: by how many
    /// times a frame pixel covers more ground than it would at nadir — only as
    /// the inverse quantity. At the silhouette rim the cosine goes to zero,
    /// while what the pixel covers grows without bound. `None` — the ray misses
    /// the Earth.
    fn grazing(&self, alpha: f64) -> Option<f64> {
        let sine = self.radius() / mesh::drawn_radius() * alpha.sin();
        (sine < 1.0).then(|| (1.0 - sine * sine).sqrt())
    }

    /// How firmly the place is held at this angle from the view axis: one —
    /// fully, zero — not held at all.
    fn grip(&self, alpha: f64) -> f64 {
        self.grazing(alpha).map_or(0.0, |cosine| (cosine * GRAZE_LIMIT).min(1.0))
    }

    /// How much arc a degree of angle costs: the point under the camera, having
    /// covered the arc, shifts the place in the frame that many times less.
    ///
    /// In the middle of the frame a degree of angle costs `r − 1` of arc, while
    /// at the silhouette rim, where the surface leaves the ray — as much as you
    /// like. `None` — the ray misses the Earth.
    fn slope_at(&self, alpha: f64) -> Option<f64> {
        let radius = self.radius() / mesh::drawn_radius();
        Some(radius * alpha.cos() / self.grazing(alpha)? - 1.0)
    }

    /// The arc from the point under the camera to the place seen at angle
    /// `alpha` to the view axis — in radians. `None` — the ray misses the Earth.
    ///
    /// A ray at this angle meets the surface where three segments converge —
    /// from the center to the camera, from the center to the point, and the ray
    /// itself; by the law of sines the angle at the point equals
    /// `asin(r · sin α)`, and the wanted arc is what remains of the unfolded
    /// angle. The principal branch of asin is valid because at the near root —
    /// the one that is seen — the angle at the camera together with the wanted
    /// arc is acute.
    ///
    /// The angle here is arbitrary, and so two parties ask this: the frame edge
    /// ([`Camera::visible_deg`]) and the held point ([`Camera::zoom_at`]).
    fn arc_at(&self, alpha: f64) -> Option<f64> {
        // To the drawn surface, not to the ellipsoid: what is seen is the lift,
        // not it. How much this matters and from what height is told at the
        // measure itself (`mesh::drawn_radius`), and told there once.
        let sine = self.radius() / mesh::drawn_radius() * alpha.sin();
        (sine < 1.0).then(|| sine.asin() - alpha)
    }

    /// How far the camera is from the center, in equatorial radii — the measure
    /// in which all the view geometry is computed.
    fn radius(&self) -> f64 {
        1.0 + self.height_m / geodesy::SEMI_MAJOR_M
    }

    /// Where the eye stands — as one number, roughly. This is enough for
    /// visibility selection: it decides whether a cell falls in the frame, and
    /// eight tenths of a meter do not change such a decision. What is drawn
    /// needs both halves ([`Camera::eye_parts`]).
    pub fn eye(&self) -> geodesy::World {
        self.eye_parts().0
    }

    /// Where the eye stands — in halves, like everything that is drawn. It is
    /// subtracted from the vertex already in the shader, and with a single half
    /// this difference would lose all the detail (see [`geodesy::parts`]).
    pub fn eye_parts(&self) -> (geodesy::World, geodesy::World) {
        geodesy::parts(geodesy::world(self.geodetic()))
    }

    /// Meters of surface per frame pixel along the vertical — the measure for
    /// choosing the level of the overlay tiles. It is computed at the center of
    /// the view: toward the frame edges the footage grows, but the level is a
    /// logarithmic quantity, and it accumulates a whole step only right at the
    /// horizon.
    pub fn metres_per_pixel(&self, height_px: u32) -> f64 {
        let arc = 2.0 * self.visible_deg().to_radians() * geodesy::SEMI_MAJOR_M;
        arc / f64::from(height_px.max(1))
    }

    /// Where the frame spot looks, in shares of the area: (0, 0) — the left
    /// top corner. Returns the ray — origin and direction; what it runs into is
    /// `geodesy`'s business.
    ///
    /// The inverse of the projection, but without inverting the matrix: the
    /// camera basis is known without it, and a 4×4 inverse is another fifty
    /// lines that would agree with the direct one only by eye.
    pub fn ray(&self, x: f32, y: f32, aspect: f32) -> (DVec3, DVec3) {
        // Area shares are counted from the corner, each on its own side; the
        // frame needs an offset from the center, both axes in shares of height.
        // The horizontal is stretched by the proportions of the area, and the
        // frame's Y axis points up.
        let (right, above) =
            frame_offset((f64::from(x) - 0.5) * f64::from(aspect), 0.5 - f64::from(y));
        (geodesy::world(self.geodetic()), self.look(right, above))
    }

    /// Where the frame spot standing off the center by the given tangents of the
    /// frame axes looks.
    fn look(&self, right: f64, above: f64) -> DVec3 {
        let (side, up, forward) = self.basis();
        (forward + side * right + up * above).normalize()
    }

    /// Where the point is seen in the frame — the same tangents, the inverse of
    /// [`Camera::look`]. `None` — the point is behind, and it has no frame spot.
    fn seen_at(&self, point: DVec3) -> Option<(f64, f64)> {
        let (side, up, forward) = self.basis();
        let direction = point - geodesy::world(self.geodetic());
        let ahead = direction.dot(forward);
        (ahead > 0.0).then(|| (direction.dot(side) / ahead, direction.dot(up) / ahead))
    }

    /// The place seen at a frame spot — the inverse of [`project`] for
    /// everything drawn on the lift.
    ///
    /// The ray meets not the surface itself but the one raised by
    /// `mesh::SURFACE_LIFT_M`: what is seen under the cursor is the drawn thing,
    /// and it is drawn there. Were the ray to cross the ellipsoid, the answer
    /// would name a place standing off the seen one: in the corner of the frame
    /// it is sixty meters at an ordinary zoom and hundreds where the surface is
    /// glanced along. The more grazing the ray, the further the extra height
    /// leads away — and right at the silhouette rim the answer is unreliable as
    /// it is (see `geodesy::intersect_at`).
    ///
    /// `None` — past the Earth; that is the same kind of answer as a point.
    pub fn probe(&self, x: f32, y: f32, aspect: f32) -> Option<(f64, f64)> {
        let (eye, direction) = self.ray(x, y, aspect);
        geodesy::intersect_at(eye, direction, mesh::SURFACE_LIFT_M).map(geodesy::surface_at)
    }

    /// The clipping planes are derived from the height, not given as numbers:
    /// on a close approach a fixed near plane eats almost all the depth
    /// precision, while at a distance the far plane would cut the Earth off.
    /// The matrix here is **without translation**: the eye stands at the origin,
    /// and the shader subtracts it from the vertex. Otherwise the translation —
    /// a number of order one — would travel to the GPU in `f32` by itself, and
    /// any vertex, however precise, would lose eight tenths of a meter in the
    /// multiplication by it.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        // Half the distance to the drawn thing, not to the ellipsoid — the same
        // measure as everything else here (`mesh::drawn_radius`). At the current
        // field it would not cut even the second one: the nadir lies at
        // `H − lift`, and `H/2` overtakes it only below two lifts, that is,
        // four times under the floor. With different measures the two would
        // silently diverge.
        let near = geodesy::metres(self.height_m - mesh::SURFACE_LIFT_M) * 0.5;
        // The far edge of the ellipsoid is no further than |eye| + semi-major
        // axis.
        let far = self.radius() as f32 + 1.05;
        // Perspective with the depth range 0..1 — the one wgpu expects (unlike
        // -1..1 of OpenGL); in glam it is `_rh`, not `_rh_gl`.
        Mat4::perspective_rh((FOV_Y_DEG as f32).to_radians(), aspect, near, far)
            * turn_to(self.basis())
    }

    /// The frame axes: right, up and forward. One for everyone who needs the
    /// camera orientation — otherwise the view matrix and the ray under the
    /// cursor would have to be derived separately, and they must agree.
    ///
    /// The view goes to the center of the ellipsoid, not along the normal, so
    /// the "up" of the frame does not coincide with the stored `up` exactly:
    /// they are parted by the tilt of the normal to the radius vector — the very
    /// 0.19° of flattening. They have nowhere to diverge — both are derived
    /// here.
    fn basis(&self) -> (DVec3, DVec3, DVec3) {
        let forward = -geodesy::world(self.geodetic()).normalize();
        let side = forward.cross(self.up).normalize();
        (side, side.cross(forward), forward)
    }
}

/// The offset of a frame spot from the center — into tangents of the frame
/// axes.
///
/// One place for everyone who needs the frame geometry: the frame stands off
/// the camera by one along the view, so half a frame vertically is the tangent
/// of half the field of view. The ray under the cursor brings area shares here,
/// zooming — height shares; computed separately, the two would silently
/// diverge, and the divergence could be seen only in the picture.
fn frame_offset(right_share: f64, above_share: f64) -> (f64, f64) {
    let half = (FOV_Y_DEG * 0.5).to_radians().tan();
    (2.0 * right_share * half, 2.0 * above_share * half)
}

/// "North up" for this point under the camera: the component of the world axis
/// orthogonal to the view. `None` — the camera is right over the pole, and no
/// such direction exists there: all meridians converge, and any turn of the
/// frame is equally good.
fn north_up(at: DVec3) -> Option<DVec3> {
    let up = DVec3::Z.reject_from_normalized(at);
    (up.length_squared() > 1e-12).then(|| up.normalize())
}

/// The top of the frame for this view direction: north, and right over the very
/// top — what is left of the previous top.
///
/// The pair must come out orthonormal, and this is not a quirk: the frame turn
/// ([`orientation`]) is derived from it, and a skewed pair has no turn — the
/// matrix from it is not orthogonal, and the quaternion from it comes out
/// unknown which.
fn frame_up(at: DVec3, previous: DVec3) -> DVec3 {
    north_up(at).unwrap_or_else(|| {
        let up = previous.reject_from_normalized(at);
        match up.length_squared() > 1e-12 {
            true => up.normalize(),
            // The previous top looked the same way as the view: over the pole
            // any turn of the frame is equally good, and the first one at hand
            // will do.
            false => at.any_orthonormal_vector(),
        }
    })
}

/// The orientation of the frame as a quaternion — the same one it is later
/// taken out of.
///
/// The columns of the matrix are the frame axes: right, up and toward the
/// camera. The frame is orthonormal (see [`Camera::settle`]), so the matrix is
/// a rotation, and a rotation has a quaternion unique up to sign, and slerp
/// does not care about the sign: it picks the short arc itself.
fn orientation(at: DVec3, up: DVec3) -> DQuat {
    DQuat::from_mat3(&DMat3::from_cols(up.cross(at), up, at))
}

/// A world point into clip space. Here, not at the one asking: the matrix
/// layout is this file's business, and a second such multiplication would once
/// diverge from it in the columns.
///
/// The eye is subtracted here, not kept in the matrix: the view matrix carries
/// no translation (see [`Camera::view_projection`]). Halves are not needed here
/// — visibility selection decides frame hits, and eight tenths of a meter at
/// the distance to the floor give a bit more than a pixel in a thousand.
///
/// Returns the four-vector as is, without dividing by w. Division is only legal
/// at `w > 0`, and it is always so here: the matrix is a pure rotation, the
/// view goes to the center of the ellipsoid, whence
/// `w = |eye| − ê·point ≥ (H − lift)/a`, that is, at the very floor there are
/// still a thousand quanta of `f32`. The caller still checks the sign itself —
/// the relation of these quantities is declared nowhere (see
/// `overlay::on_screen`).
pub fn project(view_proj: &Mat4, point: geodesy::World, eye: geodesy::World) -> Vec4 {
    *view_proj * (Vec3::from(point) - Vec3::from(eye)).extend(1.0)
}

/// The height from which a circle of this angular radius is visible — the
/// inverse of [`Camera::visible_deg`].
///
/// The vertical is taken as the measure: the field of view is set by it, while
/// the ground under the globe is wider than it is tall, so what fits vertically
/// fits across as well.
fn height_for(radius_deg: f64) -> f64 {
    let half = (FOV_Y_DEG * 0.5).to_radians();
    // No more than half a frame to the horizon is ever asked for: past this
    // mark the frame edge ray no longer comes to the Earth at all, and stepping
    // back further means seeing no more, only getting the same globe smaller —
    // at `radius_deg` of 90° the Earth would fill an eighth of the frame,
    // because the height ran into its ceiling. Right at the mark the limb of the
    // Earth sits exactly at the frame edge, and everything visible at all from
    // one side is visible.
    let arc = radius_deg.clamp(0.0, (std::f64::consts::FRAC_PI_2 - half).to_degrees())
        .to_radians();
    let radius = mesh::drawn_radius() * (arc + half).sin() / half.sin();
    (radius - 1.0) * geodesy::SEMI_MAJOR_M
}

/// The frame turn from ready axes — without translation: the world arrives at
/// the shader already counted off the eye, and there is nowhere to translate it
/// (see [`Camera::view_projection`]).
///
/// The axes go into the rows, not the columns: the view matrix takes the world
/// into the camera frame, that is, inverts its orientation, and the inverse of
/// a rotation is its transpose.
///
/// The conversion to `f32` happens here and only here: further on the matrix
/// travels to the shader, where there is no other precision.
fn turn_to((side, up, forward): (DVec3, DVec3, DVec3)) -> Mat4 {
    let rotation = DMat3::from_cols(side, up, -forward).transpose();
    DMat4::from_cols(
        rotation.x_axis.extend(0.0),
        rotation.y_axis.extend(0.0),
        rotation.z_axis.extend(0.0),
        DVec3::ZERO.extend(1.0),
    )
    .as_mat4()
}
