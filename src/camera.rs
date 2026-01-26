use glam::{DVec3, DQuat};

const EARTH_RADIUS: f64 = 6_371_000.0;

pub struct OrbitCamera {
    /// Ротор, определяющий ориентацию камеры в пространстве.
    /// Он переводит локальные оси (X-вправо, Y-вверх, Z-назад) в мировые.
    pub orientation: DQuat,
    pub distance: f64,
    pub min_distance: f64,
    pub max_distance: f64,
}

impl OrbitCamera {
    pub fn new(distance: f64, yaw: f64, pitch: f64) -> Self {
        // Начальный ротор из углов
        let rotation = DQuat::from_rotation_y(yaw) * DQuat::from_rotation_x(-pitch);
        Self {
            orientation: rotation.normalize(),
            distance,
            min_distance: EARTH_RADIUS + 10.0,
            max_distance: EARTH_RADIUS * 10.0,
        }
    }

    pub fn get_position(&self) -> DVec3 {
        // Позиция камеры в мире: поворачиваем вектор "назад" и выносим на дистанцию
        self.orientation * DVec3::new(0.0, 0.0, self.distance)
    }
}

pub struct CameraController {
    sensitivity: f64,
    is_dragged: bool,
}

impl CameraController {
    pub fn new(sensitivity: f64) -> Self {
        Self {
            // Чувствительность в радианах на пиксель (0.002 ~ 0.1 градуса)
            sensitivity, 
            is_dragged: false,
        }
    }

    pub fn process_events(&mut self, event: &winit::event::WindowEvent, camera: &mut OrbitCamera) -> bool {
        use winit::event::*;
        match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.is_dragged = *state == ElementState::Pressed;
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y as f64,
                    MouseScrollDelta::PixelDelta(pos) => pos.y / 100.0,
                };
                
                let altitude = (camera.distance - EARTH_RADIUS).max(1.0);
                let factor = 0.90f64.powf(scroll); 
                camera.distance = EARTH_RADIUS + (altitude * factor);
                camera.distance = camera.distance.clamp(camera.min_distance, camera.max_distance);
                true
            }
            _ => false,
        }
    }

    pub fn process_mouse_motion(&mut self, dx: f64, dy: f64, camera: &mut OrbitCamera) {
        if self.is_dragged {
            let altitude = (camera.distance - EARTH_RADIUS).max(1.0);
            
            // Адаптивный коэффициент: замедляем вращение при приближении к поверхности
            let zoom_scale = (altitude / EARTH_RADIUS).clamp(0.0001, 1.0);
            let delta_angle = self.sensitivity * zoom_scale;
            
            // 1. Вращение вокруг глобальной оси Y (ось планеты)
            let global_yaw = DQuat::from_rotation_y(-dx * delta_angle);
            
            // 2. Вращение вокруг локальной оси X камеры (широта)
            // Мы создаем локальный ротор и применяем его справа.
            let local_pitch = DQuat::from_rotation_x(-dy * delta_angle);
            
            // Обновляем ориентацию: Поворот_Мира * Текущая * Поворот_Локальный
            camera.orientation = global_yaw * camera.orientation * local_pitch;
            
            // Перенормировка ротора для исключения накопления ошибок
            camera.orientation = camera.orientation.normalize();
        }
    }
}