use crate::proto::ui as proto;
use veldsdk::OwnedResource;
use std::collections::HashMap;
use std::cell::RefCell;
use iced_core::{Point, Event};
use iced_runtime::user_interface;
use crate::module::renderer::GpuRenderer;

/// Response message waiting to be dispatched to plugin
#[derive(Clone)]
pub struct PendingMessage {
    pub method: String,
    pub value: String,
}

#[allow(dead_code)]
pub struct PluginUiState {
    pub layout: proto::Layout,
    pub is_layout_dirty: RefCell<bool>,
    pub interface_cache: RefCell<user_interface::Cache>,
    pub needs_redrawing: RefCell<bool>,
    pub canvas_size: RefCell<(u32, u32)>,
    pub scale_factor: RefCell<f32>,
    pub cursor_position: RefCell<Point>,
    pub scroll_velocity: RefCell<Point>,
    pub pending_events: RefCell<Vec<Event>>,
    pub vertex_buffer: RefCell<Option<OwnedResource>>,
    pub index_buffer: RefCell<Option<OwnedResource>>,
    pub uniform_buffer: RefCell<Option<OwnedResource>>,
    pub uniform_buffer_id: RefCell<Option<u64>>,
    pub uniform_layout_id: RefCell<Option<u64>>,
    pub ui_pipeline: RefCell<Option<u64>>,
    pub last_vertices: RefCell<Vec<crate::module::renderer::Vertex>>,
    pub last_draw_commands: RefCell<Vec<crate::module::renderer::DrawCmd>>,
    pub external_bind_groups: RefCell<HashMap<u64, u64>>,
    
    // Cached offscreen texture for rendering (reused if size matches)
    pub offscreen_texture: RefCell<Option<OwnedResource>>,
    pub offscreen_view: RefCell<Option<u64>>,
    pub offscreen_texture_size: RefCell<(u32, u32)>,

    // Performance Stats
    pub perf_count: RefCell<u64>,
    pub perf_total: RefCell<u128>,
    pub perf_conv: RefCell<u128>,
    pub perf_build: RefCell<u128>,
    pub perf_update: RefCell<u128>,
    pub perf_draw: RefCell<u128>,
    pub perf_gpu: RefCell<u128>,
    pub perf_last_log: RefCell<Option<std::time::Instant>>,

    pub monitor_fps: RefCell<u32>,
    pub actual_fps: RefCell<f32>,
    
    /// Messages captured from iced UI events, waiting to be dispatched
    pub pending_messages: RefCell<Vec<PendingMessage>>,
    /// Surface handle for rendering (set from Frame event)
    pub surface_handle: RefCell<Option<u64>>,
}

pub struct State {
    pub plugins: HashMap<String, PluginUiState>,
    pub renderer: GpuRenderer,
    pub surface_format: i32,
    /// Size of the single OS window/surface shared by all plugins.
    /// Tracked independently of any plugin's registration so that the very
    /// first `frame` tick can be published before any plugin has rendered
    /// (a plugin only appears in `plugins` once it has sent its first
    /// `set_view`, which itself is triggered by receiving a `frame` tick).
    pub canvas_size: (u32, u32),
}

unsafe impl Send for State {}
unsafe impl Sync for State {}

impl State {
    pub fn new() -> Self {
        let sf = veldsdk::rpc::host::get_config("surface_format")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        Self {
            plugins: HashMap::new(),
            renderer: GpuRenderer::new("JetBrains Mono", vec![
                ("JetBrains Mono", include_bytes!("../../../runtime/assets/JetBrainsMono.ttf")),
            ]),
            surface_format: sf,
            canvas_size: (0, 0),
        }
    }
}

impl PluginUiState {
    pub fn new() -> Self {
        Self {
            layout: proto::Layout::default(),
            is_layout_dirty: RefCell::new(true),
            interface_cache: RefCell::new(user_interface::Cache::default()),
            needs_redrawing: RefCell::new(true),
            canvas_size: RefCell::new((1024, 768)),
            scale_factor: RefCell::new(1.0),
            cursor_position: RefCell::new(Point::ORIGIN),
            scroll_velocity: RefCell::new(Point::ORIGIN),
            pending_events: RefCell::new(Vec::new()),
            vertex_buffer: RefCell::new(None),
            index_buffer: RefCell::new(None),
            uniform_buffer: RefCell::new(None),
            uniform_buffer_id: RefCell::new(None),
            uniform_layout_id: RefCell::new(None),
            ui_pipeline: RefCell::new(None),
            last_vertices: RefCell::new(Vec::new()),
            last_draw_commands: RefCell::new(Vec::new()),
            external_bind_groups: RefCell::new(HashMap::new()),
            offscreen_texture: RefCell::new(None),
            offscreen_view: RefCell::new(None),
            offscreen_texture_size: RefCell::new((0, 0)),
            
            perf_count: RefCell::new(0),
            perf_total: RefCell::new(0),
            perf_conv: RefCell::new(0),
            perf_build: RefCell::new(0),
            perf_update: RefCell::new(0),
            perf_draw: RefCell::new(0),
            perf_gpu: RefCell::new(0),
            perf_last_log: RefCell::new(Some(std::time::Instant::now())),
            
            monitor_fps: RefCell::new(60),
            actual_fps: RefCell::new(60.0),
            pending_messages: RefCell::new(Vec::new()),
            surface_handle: RefCell::new(None),
        }
    }
}
