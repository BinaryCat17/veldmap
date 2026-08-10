use crate::proto::ui_service as proto;
use veldsdk::OwnedResource;
use veldsdk::graphics::{BindGroupId, BindGroupLayoutId, PipelineId, TextureViewId};
use std::collections::HashMap;
use iced_core::{Point, Event};
use iced_runtime::user_interface;
use crate::module::renderer::GpuRenderer;
use crate::module::converter::UiMessage;

/// Всё, что сервис помнит об одном клиенте: его разметка, состояние ввода и
/// ресурсы устройства, живущие дольше кадра.
///
/// Поля обычные, без интерьерной изменяемости: обработчики получают `&mut
/// State`, а `plugins` и `renderer` — разные поля, и заимствовать их
/// одновременно borrow checker даёт. Кто чего касается за кадр, видно в
/// сигнатурах, и заметит расхождение компилятор, а не паника в рантайме.
pub struct PluginUiState {
    pub layout: proto::Layout,
    pub is_layout_dirty: bool,
    pub interface_cache: user_interface::Cache,
    pub needs_redrawing: bool,
    pub canvas_size: (u32, u32),
    pub scale_factor: f32,
    pub cursor_position: Point,
    pub scroll_velocity: Point,
    pub pending_events: Vec<Event>,
    /// Последнее отправленное в iced состояние модификаторов: text_input
    /// хранит modifiers у себя и обновляет их только по ModifiersChanged.
    pub keyboard_modifiers: iced_core::keyboard::Modifiers,
    pub vertex_buffer: Option<OwnedResource>,
    pub index_buffer: Option<OwnedResource>,
    /// Uniform-буфер (memory ABI): наш, освобождается Drop'ом.
    pub uniform_buffer_region: Option<OwnedResource>,
    pub uniform_bind_group: Option<BindGroupId>,
    pub uniform_layout: Option<BindGroupLayoutId>,
    pub ui_pipeline: Option<PipelineId>,
    pub last_vertices: Vec<crate::module::renderer::Vertex>,
    pub last_draw_commands: Vec<crate::module::renderer::DrawCmd>,
    pub external_bind_groups: HashMap<u64, BindGroupId>,

    /// Кэш view render-таргета: (texture_id, view).
    /// Инвалидируется сменой texture_id (владелец аллоцирует новый при resize).
    pub target_view: Option<(u64, TextureViewId)>,

    pub monitor_fps: u32,
    /// FPS-счётчик: (кадры, накопленные секунды) с последнего отчёта.
    /// Раз в 5 секунд средний FPS уходит в лог с флагом PERF.
    pub fps_window: (u32, f32),

    /// Пойманное iced'ом за этот кадр; рассылается сразу после рендера
    /// (см. handlers::render_plugin_if_needed).
    pub pending_messages: Vec<UiMessage>,
    /// Render-таргет, делегированный владельцем окна через set_surface.
    /// Не наш ресурс: освобождает его владелец окна, поэтому здесь голый id,
    /// а не OwnedResource.
    pub surface_handle: Option<u64>,
}

pub struct State {
    pub plugins: HashMap<String, PluginUiState>,
    pub renderer: GpuRenderer,
    pub surface_format: i32,
}

impl State {
    pub fn new(surface_format: i32) -> Self {
        Self {
            plugins: HashMap::new(),
            // Имена шрифтов — контракт с клиентами разметки; для них они
            // объявлены константами в veld-ui-service-wrap (style::FONT_*).
            renderer: GpuRenderer::new("JetBrains Mono", vec![
                ("JetBrains Mono", include_bytes!("../../../runtime/assets/JetBrainsMono.ttf")),
                ("Icons", include_bytes!("../../../runtime/assets/SymbolsNerdFontMono-Regular.ttf")),
            ]),
            surface_format,
        }
    }
}

impl PluginUiState {
    pub fn new() -> Self {
        Self {
            layout: proto::Layout::default(),
            is_layout_dirty: true,
            interface_cache: user_interface::Cache::default(),
            needs_redrawing: true,
            canvas_size: (1024, 768),
            scale_factor: 1.0,
            cursor_position: Point::ORIGIN,
            scroll_velocity: Point::ORIGIN,
            pending_events: Vec::new(),
            keyboard_modifiers: iced_core::keyboard::Modifiers::empty(),
            vertex_buffer: None,
            index_buffer: None,
            uniform_buffer_region: None,
            uniform_bind_group: None,
            uniform_layout: None,
            ui_pipeline: None,
            last_vertices: Vec::new(),
            last_draw_commands: Vec::new(),
            external_bind_groups: HashMap::new(),
            target_view: None,
            monitor_fps: 60,
            fps_window: (0, 0.0),
            pending_messages: Vec::new(),
            surface_handle: None,
        }
    }
}
