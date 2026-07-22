use crate::proto::ui_service as proto;
use veldsdk::OwnedResource;
use veldsdk::graphics::{BindGroupId, BindGroupLayoutId, PipelineId, TextureViewId};
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
    /// Последнее отправленное в iced состояние модификаторов: text_input
    /// хранит modifiers у себя и обновляет их только по ModifiersChanged.
    pub keyboard_modifiers: RefCell<iced_core::keyboard::Modifiers>,
    pub vertex_buffer: RefCell<Option<OwnedResource>>,
    pub index_buffer: RefCell<Option<OwnedResource>>,
    /// Region id uniform-буфера (memory ABI).
    pub uniform_buffer_region: RefCell<Option<u64>>,
    pub uniform_bind_group: RefCell<Option<BindGroupId>>,
    pub uniform_layout: RefCell<Option<BindGroupLayoutId>>,
    pub ui_pipeline: RefCell<Option<PipelineId>>,
    pub last_vertices: RefCell<Vec<crate::module::renderer::Vertex>>,
    pub last_draw_commands: RefCell<Vec<crate::module::renderer::DrawCmd>>,
    pub external_bind_groups: RefCell<HashMap<u64, BindGroupId>>,

    /// Кэш view render-таргета: (texture_id, view).
    /// Инвалидируется сменой texture_id (владелец аллоцирует новый при resize).
    pub target_view: RefCell<Option<(u64, TextureViewId)>>,

    pub monitor_fps: RefCell<u32>,
    /// FPS-счётчик: (кадры, накопленные секунды) с последнего отчёта.
    /// Раз в 5 секунд средний FPS уходит в лог с флагом PERF.
    pub fps_window: RefCell<(u32, f32)>,


    /// Messages captured from iced UI events, waiting to be dispatched
    pub pending_messages: RefCell<Vec<PendingMessage>>,
    /// Render-таргет, делегированный владельцем окна через set_surface.
    pub surface_handle: RefCell<Option<u64>>,
}

pub struct State {
    pub plugins: HashMap<String, PluginUiState>,
    pub renderer: GpuRenderer,
    pub surface_format: i32,
}

unsafe impl Send for State {}
unsafe impl Sync for State {}

impl State {
    pub fn new() -> Self {
        let sf = veldsdk::abi::get_config("surface_format")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        Self {
            plugins: HashMap::new(),
            renderer: GpuRenderer::new("JetBrains Mono", vec![
                ("JetBrains Mono", include_bytes!("../../../runtime/assets/JetBrainsMono.ttf")),
                ("Icons", include_bytes!("../../../runtime/assets/SymbolsNerdFontMono-Regular.ttf")),
            ]),
            surface_format: sf,
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
            keyboard_modifiers: RefCell::new(iced_core::keyboard::Modifiers::empty()),
            vertex_buffer: RefCell::new(None),
            index_buffer: RefCell::new(None),
            uniform_buffer_region: RefCell::new(None),
            uniform_bind_group: RefCell::new(None),
            uniform_layout: RefCell::new(None),
            ui_pipeline: RefCell::new(None),
            last_vertices: RefCell::new(Vec::new()),
            last_draw_commands: RefCell::new(Vec::new()),
            external_bind_groups: RefCell::new(HashMap::new()),
            target_view: RefCell::new(None),
            monitor_fps: RefCell::new(60),
            fps_window: RefCell::new((0, 0.0)),
            pending_messages: RefCell::new(Vec::new()),
            surface_handle: RefCell::new(None),
        }
    }
}
