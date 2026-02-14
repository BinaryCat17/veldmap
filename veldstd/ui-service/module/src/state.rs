use veld_ui::proto;
use veldsdk::OwnedResource;
use std::collections::HashMap;
use std::cell::RefCell;
use iced_core::{Point, Event};
use iced_runtime::user_interface;
use crate::renderer::GpuRenderer;

pub struct PluginUiState {
    pub layout: proto::Layout,
    pub is_layout_dirty: bool,
    pub interface_cache: RefCell<user_interface::Cache>,
    pub needs_redrawing: RefCell<bool>,
    pub canvas_size: RefCell<(u32, u32)>,
    pub scale_factor: RefCell<f32>,
    pub cursor_position: RefCell<Point>,
    pub scroll_velocity: RefCell<Point>,
    pub pending_events: RefCell<Vec<Event>>,
    pub ui_texture: RefCell<Option<OwnedResource>>,
    pub vertex_buffer: RefCell<Option<OwnedResource>>,
    pub index_buffer: RefCell<Option<OwnedResource>>,
    pub uniform_buffer: RefCell<Option<OwnedResource>>,
    pub uniform_buffer_id: RefCell<Option<u64>>,
    pub ui_pipeline: RefCell<Option<u64>>,
}

pub struct LocalState {
    pub plugins: HashMap<String, PluginUiState>,
    pub renderer: GpuRenderer,
    pub surface_format: i32,
}

unsafe impl Send for LocalState {}
unsafe impl Sync for LocalState {}

impl LocalState {
    pub fn new() -> Self {
        let sf = veldsdk::rpc::host::get_config("surface_format")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        Self {
            plugins: HashMap::new(),
            renderer: GpuRenderer::new("JetBrains Mono", vec![
                ("JetBrains Mono", include_bytes!("../../../../veldgis/assets/JetBrainsMono.ttf")),
            ]),
            surface_format: sf,
        }
    }
}

impl PluginUiState {
    pub fn new() -> Self {
        Self {
            layout: proto::Layout::default(),
            is_layout_dirty: true,
            interface_cache: RefCell::new(user_interface::Cache::default()),
            needs_redrawing: RefCell::new(true),
            canvas_size: RefCell::new((1024, 768)),
            scale_factor: RefCell::new(1.0),
            cursor_position: RefCell::new(Point::ORIGIN),
            scroll_velocity: RefCell::new(Point::ORIGIN),
            pending_events: RefCell::new(Vec::new()),
            ui_texture: RefCell::new(None),
            vertex_buffer: RefCell::new(None),
            index_buffer: RefCell::new(None),
            uniform_buffer: RefCell::new(None),
            uniform_buffer_id: RefCell::new(None),
            ui_pipeline: RefCell::new(None),
        }
    }
}
