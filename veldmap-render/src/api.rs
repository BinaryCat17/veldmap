use crate::engine::State;
use crate::camera::OrbitCamera;
use crate::tiling::TileId;
use veldmap_data::DemTile;

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

    pub fn camera_zoom(&mut self, delta: f64) {
        self.state.camera_controller.process_mouse_scroll(delta, &mut self.state.camera);
    }

    pub fn camera_move(&mut self, dx: f64, dy: f64) {
        self.state.camera_controller.process_mouse_motion(dx, dy, &mut self.state.camera);
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.state.camera
    }

    /// Загрузить данные геоида в движок.
    pub fn set_geoid(&mut self, dem: &DemTile) {
        self.state.set_geoid(dem);
    }

    /// Загрузить тайл рельефа в движок.
    pub fn upload_tile(&mut self, id: TileId, dem: &DemTile) {
        self.state.upload_tile(id, dem);
    }

    /// Получить список тайлов, которые сейчас видны камере.
    pub fn get_visible_tiles(&self) -> Vec<TileId> {
        let pos = self.state.camera.get_position();
        let altitude = pos.length() - 6_371_000.0;
        let latlon = crate::geo::cartesian_to_latlon(pos.x as f32, pos.y as f32, pos.z as f32);
        crate::tiling::get_visible_tiles(latlon.0 as f64, latlon.1 as f64, altitude)
    }
}
