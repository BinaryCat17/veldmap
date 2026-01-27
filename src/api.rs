use crate::engine::State;
use crate::camera::OrbitCamera;

/// Главный интерфейс библиотеки VeldMap.
pub struct VeldMap {
    state: State,
}

impl VeldMap {
    pub async fn new<W>(window: W) -> Self 
    where W: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle + Send + Sync + 'static 
    {
        let state = State::new(window).await;
        Self { state }
    }

    pub fn update(&mut self) {
        self.state.update();
    }

    pub fn render(&mut self) -> Result<(), String> {
        self.state.render().map_err(|e| format!("{:?}", e))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.state.resize(width, height);
    }

    /// Приблизить/удалить камеру.
    pub fn camera_zoom(&mut self, delta: f64) {
        self.state.camera_controller.process_mouse_scroll(delta, &mut self.state.camera);
    }

    /// Повернуть камеру.
    pub fn camera_move(&mut self, dx: f64, dy: f64) {
        self.state.camera_controller.process_mouse_motion(dx, dy, &mut self.state.camera);
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.state.camera
    }
}