use crate::proto::ui_service as proto;
use veldsdk::OwnedResource;
use veldsdk::graphics::{
    buffer_usage, BindGroupId, BindGroupLayoutId, GrowingBuffer, PipelineId, TextureViewId,
};
use std::collections::HashMap;
use iced_core::{Point, Event};
use iced_runtime::user_interface;
use crate::module::renderer::GpuRenderer;
use crate::module::converter::UiMessage;

/// С чего начинаются буферы геометрии. Дальше они растут под кадр, поэтому это
/// не потолок, а «столько понадобится точно»: обычный экран разметки
/// укладывается в мегабайт и не перевыделяет буфер ни разу.
const GEOMETRY_BUFFER: u64 = 1024 * 1024;

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
    /// Когда виджеты просят нарисовать себя снова сами по себе: мигающая
    /// каретка ввода, анимация. Просьбу называет iced ответом `ui.update`
    /// (`RedrawRequest`), и без неё каретка стои́т — разметка-то не меняется, а
    /// по ней одной об устаревании кадра не узнать.
    ///
    /// `None` — никто ничего не просил. Мгновение прошлое означает «пора»:
    /// кадровый тик сравнит его со своим временем (см. `handle_ui_event`).
    pub redraw_at: Option<std::time::Instant>,
    pub pending_events: Vec<Event>,
    /// Последнее отправленное в iced состояние модификаторов: text_input
    /// хранит modifiers у себя и обновляет их только по ModifiersChanged.
    pub keyboard_modifiers: iced_core::keyboard::Modifiers,
    /// Геометрия кадра: растёт под то, сколько на экране строк и глифов.
    pub vertex_buffer: GrowingBuffer,
    pub index_buffer: GrowingBuffer,
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
    /// Темп разбора кадров, раз в несколько секунд в trace.log.
    pub frames: crate::module::frames::FrameMeter,

    /// По какой просьбе уже наведены области прокрутки: имя области → номер
    /// применённой просьбы. Нужно затем, что наводка одноразовая: разметка
    /// пересобирается на каждое событие, и наводка, применяемая каждый кадр,
    /// держала бы прокрутку намертво (см. `Scrollable.scroll_to`).
    pub aimed: HashMap<String, u64>,

    /// Незакрытый вопрос платформы «где на экране этот элемент». Отвечается в
    /// том же кадре, в котором пришёл, и снимается ответом: обход стои́т
    /// раскладки, а раскладка бывает только внутри кадра (см. locate.rs).
    pub pending_locate: Option<veldsdk::proto::app::LocateWidget>,

    /// Пойманное iced'ом за этот кадр; рассылается сразу после рендера
    /// (см. handlers::render_plugin_if_needed).
    pub pending_messages: Vec<UiMessage>,
    /// Render-таргет, делегированный владельцем через set_surface.
    /// Не наш ресурс: освобождает его владелец, поэтому здесь голый id,
    /// а не OwnedResource.
    pub surface_handle: Option<u64>,
    /// Формат делегированной текстуры — под него собран `ui_pipeline`.
    /// Приезжает вместе с самой текстурой и хранится рядом с ней: у разных
    /// клиентов места под рендер разные, и общего формата у них нет.
    pub surface_format: i32,
}

pub struct State {
    pub plugins: HashMap<String, PluginUiState>,
    pub renderer: GpuRenderer,
}

impl State {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            // Имена шрифтов — контракт с клиентами разметки; для них они
            // объявлены константами в veld-ui-service-wrap (style::FONT_*).
            //
            // У одного имени бывает несколько файлов: начертания — это разные
            // лица одного семейства, и выбирает между ними уже вес текста
            // (Text.weight). Без своего файла вес подменяется ближайшим.
            renderer: GpuRenderer::new("UI", vec![
                ("UI", include_bytes!("../../../runtime/assets/AlegreyaSans-Regular.ttf")),
                ("UI", include_bytes!("../../../runtime/assets/AlegreyaSans-Medium.ttf")),
                ("UI", include_bytes!("../../../runtime/assets/AlegreyaSans-Bold.ttf")),
                ("Mono", include_bytes!("../../../runtime/assets/JetBrainsMono.ttf")),
                ("Icons", include_bytes!("../../../runtime/assets/SymbolsNerdFontMono-Regular.ttf")),
            ]),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
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
            redraw_at: None,
            pending_events: Vec::new(),
            keyboard_modifiers: iced_core::keyboard::Modifiers::empty(),
            vertex_buffer: GrowingBuffer::new(buffer_usage::VERTEX, "вершины", GEOMETRY_BUFFER),
            index_buffer: GrowingBuffer::new(buffer_usage::INDEX, "индексы", GEOMETRY_BUFFER),
            uniform_buffer_region: None,
            uniform_bind_group: None,
            uniform_layout: None,
            ui_pipeline: None,
            last_vertices: Vec::new(),
            last_draw_commands: Vec::new(),
            external_bind_groups: HashMap::new(),
            target_view: None,
            monitor_fps: 60,
            frames: crate::module::frames::FrameMeter::new(),
            aimed: HashMap::new(),
            pending_locate: None,
            pending_messages: Vec::new(),
            surface_handle: None,
            surface_format: 0,
        }
    }

    /// Разметка сменилась. Оба флага — одним движением: `is_layout_dirty`
    /// гасится диффом геометрии, `needs_redrawing` — самим рендером, и
    /// поставленные порознь они однажды рассогласуются (перерисовка обязана
    /// следовать за сменой разметки всегда).
    pub fn set_layout(&mut self, layout: proto::Layout) {
        self.layout = layout;
        self.is_layout_dirty = true;
        self.needs_redrawing = true;
    }
}
