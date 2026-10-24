use glam::Vec3;

use super::camera::Mat4;
use super::geodesy::{self, World};

#[derive(Clone, Copy, Debug)]
pub struct Ball {
    pub centre: [f32; 3],
    pub radius: f32,
}

impl Ball {
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

    pub fn widened(self, by: f32) -> Self {
        Self { radius: self.radius + by, ..self }
    }

}

fn distance(from: [f32; 3], to: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub struct Frame {
    inward: [Vec3; 4],
    eye: Vec3,
    towards: Vec3,
    horizon: f32,
}

impl Frame {
    pub fn new(view_proj: &Mat4, eye: World) -> Self {
        let (row0, row1, row3) = (view_proj.row(0), view_proj.row(1), view_proj.row(3));
        let inward = [row3 + row0, row3 - row0, row3 + row1, row3 - row1]
            .map(|plane| plane.truncate().normalize());
        let eye = Vec3::from(eye);
        let towards = eye.normalize();
        let polar = (1.0 - geodesy::FLATTENING) as f32;
        Self { inward, eye, towards, horizon: polar * polar / eye.length() }
    }

    pub fn admits(&self, ball: &Ball) -> bool {
        let centre = Vec3::from(ball.centre);
        let from_eye = centre - self.eye;
        if centre.dot(self.towards) + ball.radius < self.horizon {
            return false;
        }
        self.inward.iter().all(|inward| inward.dot(from_eye) + ball.radius >= 0.0)
    }
}
