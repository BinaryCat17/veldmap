use glam::{DMat3, DMat4, DQuat, DVec3, Vec3, Vec4};

pub use glam::Mat4;

use crate::module::geodesy::{self, Geodetic};
use crate::module::mesh;

const FOV_Y_DEG: f64 = 50.0;

use super::wheel::ZOOM_PER_STEP;

const HEIGHT_RANGE_M: (f64, f64) = (mesh::SURFACE_LIFT_M * 10.0, 80_000_000.0);

const GRAZE_LIMIT: f64 = 2.0;

const FRAME_MARGIN: f64 = 1.3;

const FLIGHT_S: f64 = 0.6;

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
    at: DVec3,
    up: DVec3,
    height_m: f64,
    flight: Option<Flight>,
}

impl Default for Camera {
    fn default() -> Self {
        let at = geodesy::unit(45.0, 60.0);
        Self { at, up: frame_up(at, DVec3::Z), height_m: 14_000_000.0, flight: None }
    }
}

impl Camera {
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.flight = None;

        let span = 2.0 * self.visible_deg();
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

    pub fn zoom_at(&mut self, steps: f32, hold_x: f32, hold_y: f32) {
        if !(steps.is_finite() && hold_x.is_finite() && hold_y.is_finite()) {
            return;
        }
        self.flight = None;

        let (right, above) = frame_offset(f64::from(hold_x), -f64::from(hold_y));

        let height_m = (self.height_m * ZOOM_PER_STEP.powf(f64::from(steps)))
            .clamp(HEIGHT_RANGE_M.0, HEIGHT_RANGE_M.1);
        let alpha_seen = right.hypot(above).atan();
        let share = self.grip(alpha_seen).min(Self { height_m, ..*self }.grip(alpha_seen));
        let (right, above) = (right * share, above * share);
        let alpha = right.hypot(above).atan();
        let held = geodesy::intersect_at(
            geodesy::world(self.geodetic()),
            self.look(right, above),
            mesh::SURFACE_LIFT_M,
        );
        let before = self.arc_at(alpha);

        let moved = height_m != self.height_m;
        self.height_m = height_m;

        let Some(held) = held.filter(|_| moved) else { return };

        if let (Some(before), Some(after)) = (before, self.arc_at(alpha)) {
            let (side, up, _) = self.basis();
            self.swing(side * right + up * above, before - after);
        }

        let Some(slope) = self.slope_at(alpha) else { return };
        let across_slope = match self.arc_at(alpha) {
            Some(arc) if alpha > 0.0 => arc.tan() / alpha,
            _ => slope,
        };
        // The hold is corrected by iteration: the arc is spherical, while the frame
        // turns about the ellipsoid normal. One pass leaves about a pixel of drift
        // per click, and each next click enlarges what has already moved — so the
        // held place is asked again and the frame turned by what it still misses.
        let length = right.hypot(above);
        for _ in 0..2 {
            let Some((was_right, was_above)) = self.seen_at(held) else { return };
            let now = was_right.hypot(was_above);
            let (along, across) = match length > 0.0 && now > 0.0 {
                true => (
                    now.atan() * (right * was_right + above * was_above) / (length * now) - alpha,
                    now.atan() * (right * was_above - above * was_right) / (length * now),
                ),
                false => (now.atan() - alpha, 0.0),
            };

            let (side, up, _) = self.basis();
            let (toward, sideways) = match length > 0.0 {
                true => (
                    (side * right + up * above) / length,
                    (up * right - side * above) / length,
                ),
                false => match now > 0.0 {
                    true => ((side * was_right + up * was_above) / now, up),
                    false => (side, up),
                },
            };
            self.swing(toward, along * slope);
            self.swing(sideways, across * across_slope);
        }
    }

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

    pub fn focus(&mut self, lat_deg: f64, lon_deg: f64, radius_deg: f64) {
        let at = geodesy::unit(lat_deg, lon_deg);
        let up = frame_up(at, self.up);
        let height_m =
            height_for(radius_deg * FRAME_MARGIN).clamp(HEIGHT_RANGE_M.0, HEIGHT_RANGE_M.1);

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

    pub fn advance(&mut self, dt_s: f32) -> bool {
        let Some(mut flight) = self.flight else { return false };

        flight.passed_s += f64::from(dt_s).max(0.0);
        let part = (flight.passed_s / FLIGHT_S).clamp(0.0, 1.0);
        let eased = part * part * (3.0 - 2.0 * part);

        let spin = flight.from.slerp(flight.to, eased);
        self.at = spin * DVec3::Z;
        self.up = spin * DVec3::Y;
        let times = flight.to_height_m / flight.from_height_m;
        self.height_m = flight.from_height_m * times.powf(eased);

        match part >= 1.0 {
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

    fn geodetic(&self) -> Geodetic {
        let (lat_deg, lon_deg) = geodesy::angles(self.at);
        Geodetic { lat_deg, lon_deg, height_m: self.height_m }
    }

    fn side(&self) -> DVec3 {
        self.up.cross(self.at).normalize()
    }

    fn settle(&mut self) {
        self.at = self.at.normalize();
        let up = self.up.reject_from_normalized(self.at);
        if up.length_squared() > 1e-12 {
            self.up = up.normalize();
        }
    }

    fn visible_deg(&self) -> f64 {
        match self.arc_at((FOV_Y_DEG * 0.5).to_radians()) {
            Some(arc) => arc.to_degrees(),
            None => (mesh::drawn_radius() / self.radius()).acos().to_degrees(),
        }
    }

    fn grazing(&self, alpha: f64) -> Option<f64> {
        let sine = self.radius() / mesh::drawn_radius() * alpha.sin();
        (sine < 1.0).then(|| (1.0 - sine * sine).sqrt())
    }

    fn grip(&self, alpha: f64) -> f64 {
        self.grazing(alpha).map_or(0.0, |cosine| (cosine * GRAZE_LIMIT).min(1.0))
    }

    fn slope_at(&self, alpha: f64) -> Option<f64> {
        let radius = self.radius() / mesh::drawn_radius();
        Some(radius * alpha.cos() / self.grazing(alpha)? - 1.0)
    }

    fn arc_at(&self, alpha: f64) -> Option<f64> {
        let sine = self.radius() / mesh::drawn_radius() * alpha.sin();
        (sine < 1.0).then(|| sine.asin() - alpha)
    }

    fn radius(&self) -> f64 {
        1.0 + self.height_m / geodesy::SEMI_MAJOR_M
    }

    pub fn eye(&self) -> geodesy::World {
        self.eye_parts().0
    }

    pub fn eye_parts(&self) -> (geodesy::World, geodesy::World) {
        geodesy::parts(geodesy::world(self.geodetic()))
    }

    pub fn metres_per_pixel(&self, height_px: u32) -> f64 {
        let arc = 2.0 * self.visible_deg().to_radians() * geodesy::SEMI_MAJOR_M;
        arc / f64::from(height_px.max(1))
    }

    pub fn ray(&self, x: f32, y: f32, aspect: f32) -> (DVec3, DVec3) {
        let (right, above) =
            frame_offset((f64::from(x) - 0.5) * f64::from(aspect), 0.5 - f64::from(y));
        (geodesy::world(self.geodetic()), self.look(right, above))
    }

    fn look(&self, right: f64, above: f64) -> DVec3 {
        let (side, up, forward) = self.basis();
        (forward + side * right + up * above).normalize()
    }

    fn seen_at(&self, point: DVec3) -> Option<(f64, f64)> {
        let (side, up, forward) = self.basis();
        let direction = point - geodesy::world(self.geodetic());
        let ahead = direction.dot(forward);
        (ahead > 0.0).then(|| (direction.dot(side) / ahead, direction.dot(up) / ahead))
    }

    pub fn probe(&self, x: f32, y: f32, aspect: f32) -> Option<(f64, f64)> {
        let (eye, direction) = self.ray(x, y, aspect);
        geodesy::intersect_at(eye, direction, mesh::SURFACE_LIFT_M).map(geodesy::surface_at)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let near = geodesy::metres(self.height_m - mesh::SURFACE_LIFT_M) * 0.5;
        let far = self.radius() as f32 + 1.05;
        Mat4::perspective_rh((FOV_Y_DEG as f32).to_radians(), aspect, near, far)
            * turn_to(self.basis())
    }

    fn basis(&self) -> (DVec3, DVec3, DVec3) {
        let forward = -geodesy::world(self.geodetic()).normalize();
        let side = forward.cross(self.up).normalize();
        (side, side.cross(forward), forward)
    }
}

fn frame_offset(right_share: f64, above_share: f64) -> (f64, f64) {
    let half = (FOV_Y_DEG * 0.5).to_radians().tan();
    (2.0 * right_share * half, 2.0 * above_share * half)
}

fn north_up(at: DVec3) -> Option<DVec3> {
    let up = DVec3::Z.reject_from_normalized(at);
    (up.length_squared() > 1e-12).then(|| up.normalize())
}

fn frame_up(at: DVec3, previous: DVec3) -> DVec3 {
    north_up(at).unwrap_or_else(|| {
        let up = previous.reject_from_normalized(at);
        match up.length_squared() > 1e-12 {
            true => up.normalize(),
            false => at.any_orthonormal_vector(),
        }
    })
}

fn orientation(at: DVec3, up: DVec3) -> DQuat {
    DQuat::from_mat3(&DMat3::from_cols(up.cross(at), up, at))
}

pub fn project(view_proj: &Mat4, point: geodesy::World, eye: geodesy::World) -> Vec4 {
    *view_proj * (Vec3::from(point) - Vec3::from(eye)).extend(1.0)
}

fn height_for(radius_deg: f64) -> f64 {
    let half = (FOV_Y_DEG * 0.5).to_radians();
    let arc = radius_deg.clamp(0.0, (std::f64::consts::FRAC_PI_2 - half).to_degrees())
        .to_radians();
    let radius = mesh::drawn_radius() * (arc + half).sin() / half.sin();
    (radius - 1.0) * geodesy::SEMI_MAJOR_M
}

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
