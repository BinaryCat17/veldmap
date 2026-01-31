use glam::{Mat4, Vec3, Quat};
const EARTH_RADIUS: f64 = 6378137.0;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct OrbitCamera {
    pub orientation: DQuat,
    pub distance: f64,
    pub min_distance: f64,
    pub max_distance: f64,
}

impl OrbitCamera {
    pub fn new(distance: f64, yaw: f64, pitch: f64) -> Self {
        let rotation = DQuat::from_rotation_y(yaw) * DQuat::from_rotation_x(-pitch);
        Self {
            orientation: rotation.normalize(),
            distance,
            min_distance: EARTH_RADIUS + 10.0,
            max_distance: EARTH_RADIUS * 10.0,
        }
    }

    pub fn get_position(&self) -> DVec3 {
        // Здесь Y-up система координат для рендерера
        self.orientation * DVec3::new(0.0, 0.0, self.distance)
    }
}

pub struct CameraController {
    sensitivity: f64,
}

impl CameraController {
    pub fn new(sensitivity: f64) -> Self {
        Self { sensitivity }
    }

    pub fn process_mouse_scroll(&self, scroll_delta: f64, camera: &mut OrbitCamera) {
        let altitude = (camera.distance - EARTH_RADIUS).max(1.0);
        let factor = 0.90f64.powf(scroll_delta);
        camera.distance = EARTH_RADIUS + (altitude * factor);
        camera.distance = camera.distance.clamp(camera.min_distance, camera.max_distance);
    }

    pub fn process_mouse_motion(&self, dx: f64, dy: f64, camera: &mut OrbitCamera) {
        let altitude = (camera.distance - EARTH_RADIUS).max(1.0);
        let zoom_scale = (altitude / EARTH_RADIUS).clamp(0.0001, 1.0);
        let delta_angle = self.sensitivity * zoom_scale;

        let global_yaw = DQuat::from_rotation_y(-dx * delta_angle);
        let local_pitch = DQuat::from_rotation_x(-dy * delta_angle);

        camera.orientation = global_yaw * camera.orientation * local_pitch;
        camera.orientation = camera.orientation.normalize();
    }
}