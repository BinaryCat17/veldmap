use iced_core::{Transformation, Size, Point, Pixels, Font, Color};
use iced_core::text::{LineHeight, Highlighter, highlighter, Paragraph};
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping};
use std::collections::HashMap;

use std::cell::RefCell;
use std::ptr::NonNull;

thread_local! {
    static FONT_SYSTEM: RefCell<Option<NonNull<FontSystem>>> = const { RefCell::new(None) };
    static SWASH_CACHE: RefCell<Option<NonNull<SwashCache>>> = const { RefCell::new(None) };
    static FONT_MAP: RefCell<Option<NonNull<HashMap<String, String>>>> = const { RefCell::new(None) };
    static DEFAULT_FAMILY: RefCell<Option<NonNull<String>>> = const { RefCell::new(None) };
}

pub struct ScopeGuard;
impl ScopeGuard {
    pub fn new(fs: &mut FontSystem, sc: &mut SwashCache, fm: &HashMap<String, String>, df: &String) -> Self {
        FONT_SYSTEM.with(|f| *f.borrow_mut() = Some(NonNull::from(fs)));
        SWASH_CACHE.with(|s| *s.borrow_mut() = Some(NonNull::from(sc)));
        FONT_MAP.with(|m| *m.borrow_mut() = Some(NonNull::from(fm)));
        DEFAULT_FAMILY.with(|d| *d.borrow_mut() = Some(NonNull::from(df)));
        Self
    }
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        FONT_SYSTEM.with(|f| *f.borrow_mut() = None);
        SWASH_CACHE.with(|s| *s.borrow_mut() = None);
        FONT_MAP.with(|m| *m.borrow_mut() = None);
        DEFAULT_FAMILY.with(|d| *d.borrow_mut() = None);
    }
}

/// Раскладывает строку в буфер cosmic-text. Единственный вызывающий —
/// `build_paragraph`, и через него на неё выходят оба пути текста.
///
/// `family` — уже разрешённое семейство (см. `resolve_family`); пустая строка
/// означает «разрешить не удалось», и тогда нужен именно `SansSerif`:
/// `Family::Name("")` не совпадает ни с чем.
fn shape_text(
    font_system: &mut FontSystem,
    content: &str,
    size: Pixels,
    line_height: LineHeight,
    bounds_width: f32,
    family: &str,
    shaping: iced_core::text::Shaping,
    origin: &str,
) -> Buffer {
    let mut size = size.0;
    let mut line_height = line_height.to_absolute(Pixels(size)).0;

    if size <= 0.0 {
        veldsdk::log::error!(target: "handlers", "Cosmic-text {}: invalid font size: {}. Resetting to 16.0", origin, size);
        size = 16.0;
    }
    if line_height <= 0.0 {
        veldsdk::log::error!(target: "handlers", "Cosmic-text {}: invalid line height: {}. Resetting to {}", origin, line_height, size * 1.2);
        line_height = size * 1.2;
    }

    let mut buffer = Buffer::new(font_system, Metrics::new(size, line_height));

    if bounds_width.is_finite() {
        buffer.set_size(font_system, Some(bounds_width), None);
    }

    let attrs = if family.is_empty() {
        cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif)
    } else {
        cosmic_text::Attrs::new().family(cosmic_text::Family::Name(family))
    };

    let shaping = match shaping {
        iced_core::text::Shaping::Basic => Shaping::Basic,
        iced_core::text::Shaping::Advanced => Shaping::Advanced,
    };

    buffer.set_text(font_system, content, attrs, shaping);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Переводит запрошенное iced-семейство в реальное семейство из fontdb.
/// Логические имена (как при загрузке в `GpuRenderer::new`) идут через
/// `font_map`, реальные имена семейств — напрямую, всё остальное с
/// предупреждением заменяется на дефолтное семейство.
fn resolve_family(
    db: &cosmic_text::fontdb::Database,
    font_map: &HashMap<String, String>,
    default_family: &str,
    family: &iced_core::font::Family,
) -> String {
    match family {
        iced_core::font::Family::Name(name) => {
            if let Some(mapped) = font_map.get(*name) {
                mapped.clone()
            } else if db.faces().any(|f| f.families.iter().any(|(n, _)| n.as_str() == *name)) {
                name.to_string()
            } else {
                veldsdk::log::warn!(target: "handlers", "Cosmic-text: unknown font family '{}', falling back to '{}'", name, default_family);
                default_family.to_string()
            }
        }
        _ => default_family.to_string(),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    pub uv: [f32; 2],
    pub local_pos: [f32; 2],
    pub rect_size: [f32; 2],
    pub radius: f32,
    pub mode: f32, 
    pub border_width: f32,
    pub border_color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GlyphInfo {
    uv: [f32; 4],
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawCmd {
    Quads {
        count: u32,
    },
    Scissor {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    ExternalImage {
        bounds: iced_core::Rectangle,
        texture_id: u64,
        index_count: u32,
    },
}

pub struct GpuRenderer {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
    pub draw_commands: Vec<DrawCmd>,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    /// Текстура атласа (memory ABI): наша, освобождается Drop'ом.
    pub atlas_texture_id: Option<veldsdk::OwnedResource>,
    pub atlas_bind_group: Option<veldsdk::graphics::BindGroupId>,
    pub atlas_layout: Option<veldsdk::graphics::BindGroupLayoutId>,
    glyph_cache: HashMap<cosmic_text::CacheKey, GlyphInfo>,
    atlas_data: Vec<u8>,
    atlas_width: u32,
    atlas_height: u32,
    current_atlas_x: u32,
    current_atlas_y: u32,
    row_height: u32,
    atlas_dirty_y_min: u32,
    atlas_dirty_y_max: u32,
    font_map: HashMap<String, String>,
    default_family: String,
    pub current_sf: f32,
    scissor_stack: Vec<iced_core::Rectangle>,
    transformation_stack: Vec<Transformation>,
    pub current_width: u32,
    pub current_height: u32,
}

impl GpuRenderer {
    pub fn new(default_font_name: &str, fonts: Vec<(&str, &[u8])>) -> Self {
        let mut font_map = HashMap::new();
        let mut font_system = FontSystem::new();

        for (name, data) in fonts {
            let source = cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(data.to_vec()));
            let ids = font_system.db_mut().load_font_source(source);
            if let Some(first_id) = ids.first() {
                if let Some(info) = font_system.db().face(*first_id) {
                    font_map.insert(name.to_string(), info.families[0].0.clone());
                }
            }
        }

        let default_family = match font_map.get(default_font_name) {
            Some(family) => family.clone(),
            None => {
                veldsdk::log::error!(target: "handlers", "Default font '{}' was not loaded; text rendering will fall back to the first available font", default_font_name);
                font_map.values().next().cloned().unwrap_or_else(|| default_font_name.to_string())
            }
        };

        Self {
            vertices: Vec::with_capacity(8192),
            indices: Vec::with_capacity(12288),
            draw_commands: Vec::new(),
            font_system,
            swash_cache: SwashCache::new(),
            atlas_texture_id: None,
            atlas_bind_group: None,
            atlas_layout: None,
            glyph_cache: HashMap::new(),
            atlas_width: 2048,
            atlas_height: 2048,
            atlas_data: {
                let mut data = vec![0; 2048 * 2048 * 4];
                for y in 0..4 {
                    for x in 0..4 {
                        let idx = (y * 2048 + x) * 4;
                        for i in 0..4 {
                            data[idx + i] = 255;
                        }
                    }
                }
                data
            },
            current_atlas_x: 6,
            current_atlas_y: 6,
            row_height: 1,
            atlas_dirty_y_min: 0,
            atlas_dirty_y_max: 2048,
            font_map,
            default_family,
            current_sf: 1.0,
            scissor_stack: Vec::new(),
            transformation_stack: vec![Transformation::IDENTITY],
            current_width: 0,
            current_height: 0,
        }
    }

    /// Устанавливает thread-local контекст для `RealParagraph::with_text`
    /// (шейпинг идёт через статический метод трейта без доступа к `self`).
    pub fn scope_guard(&mut self) -> ScopeGuard {
        ScopeGuard::new(&mut self.font_system, &mut self.swash_cache, &self.font_map, &self.default_family)
    }

    /// Сброс кадра: геометрия, команды и стеки состояния. Единственная точка
    /// сброса — её же зовёт `iced_core::Renderer::clear`.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.draw_commands.clear();
        self.scissor_stack.clear();
        self.transformation_stack.clear();
        self.transformation_stack.push(Transformation::IDENTITY);
    }

    pub fn update_params(&mut self, width: u32, height: u32, sf: f32) {
        self.current_sf = sf;
        self.current_width = width;
        self.current_height = height;
    }

    pub fn atlas_dimensions(&self) -> (u32, u32) {
        (self.atlas_width, self.atlas_height)
    }

    /// Атлас грузится целиком: dzn (Vulkan поверх DX12 в WSL) не принимает
    /// частичную запись текстуры. Диапазон `atlas_dirty_y_*` поэтому отвечает
    /// только на вопрос «менялся ли атлас» (`is_atlas_dirty`), а оконная
    /// выгрузка по нему была вторым, никем не вызываемым путём.
    pub fn atlas_data_full(&self) -> &[u8] {
        &self.atlas_data
    }

    pub fn is_atlas_dirty(&self) -> bool {
        self.atlas_dirty_y_min < self.atlas_dirty_y_max
    }

    pub fn mark_atlas_clean(&mut self) {
        self.atlas_dirty_y_min = self.atlas_height;
        self.atlas_dirty_y_max = 0;
    }

    pub fn mark_atlas_dirty(&mut self) {
        self.atlas_dirty_y_min = 0;
        self.atlas_dirty_y_max = self.atlas_height;
    }

    fn transform_rect(&self, rect: [f32; 4]) -> [f32; 4] {
        if let Some(t) = self.transformation_stack.last() {
            let p1 = Point::new(rect[0], rect[1]) * *t;
            let p2 = Point::new(rect[0] + rect[2], rect[1] + rect[3]) * *t;
            [p1.x, p1.y, p2.x - p1.x, p2.y - p1.y]
        } else {
            rect
        }
    }

    pub fn add_quad(&mut self, rect: [f32; 4], color: [f32; 4], uv: [f32; 4], radius: f32, mode: f32, border_width: f32, border_color: [f32; 4]) {
        self.push_quad_data(rect, color, uv, radius, mode, border_width, border_color);

        match self.draw_commands.last_mut() {
            Some(DrawCmd::Quads { count }) => *count += 6,
            _ => self.draw_commands.push(DrawCmd::Quads { count: 6 }),
        }
    }

    fn push_quad_data(&mut self, rect: [f32; 4], color: [f32; 4], uv: [f32; 4], radius: f32, mode: f32, border_width: f32, border_color: [f32; 4]) {
        let transformed = self.transform_rect(rect);
        let x = transformed[0];
        let y = transformed[1];
        let w = transformed[2];
        let h = transformed[3];
        let u1 = uv[0];
        let v1 = uv[1];
        let u2 = uv[2];
        let v2 = uv[3];

        let rw = rect[2];
        let rh = rect[3];

        let v = |px: f32, py: f32, u: f32, v: f32, lx: f32, ly: f32| Vertex {
            pos: [px, py],
            color,
            uv: [u, v],
            local_pos: [lx, ly],
            rect_size: [rw, rh],
            radius,
            mode,
            border_width,
            border_color,
        };

        let base = self.vertices.len() as u16;
        self.vertices.push(v(x, y, u1, v1, 0.0, 0.0));
        self.vertices.push(v(x + w, y, u2, v1, rw, 0.0));
        self.vertices.push(v(x + w, y + h, u2, v2, rw, rh));
        self.vertices.push(v(x, y + h, u1, v2, 0.0, rh));

        self.indices.push(base);
        self.indices.push(base + 1);
        self.indices.push(base + 2);
        self.indices.push(base);
        self.indices.push(base + 2);
        self.indices.push(base + 3);
    }

    pub fn draw_wgpu_image(&mut self, bounds: iced_core::Rectangle, texture_id: u64) {
        // Mode 2.0 = External Image mode in shaders.wgsl
        // UV [0,0, 1,1] = Full texture
        self.push_quad_data(
            [bounds.x, bounds.y, bounds.width, bounds.height],
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            0.0, 2.0, 0.0, [0.0, 0.0, 0.0, 0.0]
        );

        self.draw_commands
            .push(DrawCmd::ExternalImage { bounds, texture_id, index_count: 6 });
    }}

impl iced_widget::core::renderer::Renderer for GpuRenderer {
    fn fill_quad(
        &mut self,
        quad: iced_widget::core::renderer::Quad,
        background: impl Into<iced_core::Background>,
    ) {
        let color = match background.into() {
            iced_core::Background::Color(c) => [c.r, c.g, c.b, c.a],
            _ => [1.0, 1.0, 1.0, 1.0],
        };

        let white_uv = [2.0 / 2048.0, 2.0 / 2048.0, 2.0 / 2048.0, 2.0 / 2048.0];
        let radius = quad.border.radius.top_left;
        
        let bw = quad.border.width;
        let bc = quad.border.color;
        let border_color = [bc.r, bc.g, bc.b, bc.a];

        // РИСУЕМ ВСЁ ЗА ОДИН ПРОХОД (Фон + Рамка)
        self.add_quad(
            [
                quad.bounds.x,
                quad.bounds.y,
                quad.bounds.width,
                quad.bounds.height,
            ],
            color,
            white_uv,
            radius,
            1.0, // SDF Mode
            bw,
            border_color,
        );
    }
    fn clear(&mut self) {
        GpuRenderer::clear(self);
    }
    fn start_layer(&mut self, bounds: iced_core::Rectangle) {
        self.scissor_stack.push(bounds);
        self.apply_scissor();
    }
    fn end_layer(&mut self) {
        self.scissor_stack.pop();
        self.apply_scissor();
    }
    fn start_transformation(&mut self, transformation: Transformation) {
        let current = self
            .transformation_stack
            .last()
            .cloned()
            .unwrap_or(Transformation::IDENTITY);
        self.transformation_stack.push(current * transformation);
    }
    fn end_transformation(&mut self) {
        self.transformation_stack.pop();
    }
}

impl GpuRenderer {
    fn apply_scissor(&mut self) {
        if let Some(rect) = self.scissor_stack.last() {
            let x = (rect.x * self.current_sf).max(0.0) as u32;
            let y = (rect.y * self.current_sf).max(0.0) as u32;
            let w = (rect.width * self.current_sf) as u32;
            let h = (rect.height * self.current_sf) as u32;
            self.draw_commands
                .push(DrawCmd::Scissor { x, y, width: w, height: h });
        } else {
            self.draw_commands.push(DrawCmd::Scissor {
                x: 0,
                y: 0,
                width: self.current_width,
                height: self.current_height,
            });
        }
    }
}

impl iced_core::image::Renderer for GpuRenderer {
    type Handle = iced_core::image::Handle;
    fn measure_image(&self, _handle: &iced_core::image::Handle) -> Size<u32> {
        Size::new(100, 100)
    }
    fn draw_image(&mut self, _handle: iced_core::Image, _at: iced_core::Rectangle) {}
}

pub struct RealParagraph {
    pub buffer: Option<Buffer>,
    pub horizontal_alignment: iced_core::alignment::Horizontal,
    pub vertical_alignment: iced_core::alignment::Vertical,
    pub bounds: Size,
    pub min_x: f32,
    pub max_x: f32,
    pub visual_height: f32,
}

impl Default for RealParagraph {
    fn default() -> Self {
        Self {
            buffer: None,
            horizontal_alignment: iced_core::alignment::Horizontal::Left,
            vertical_alignment: iced_core::alignment::Vertical::Top,
            bounds: Size::INFINITY,
            min_x: 0.0,
            max_x: 0.0,
            visual_height: 0.0,
        }
    }
}

/// Шейпит текст и меряет его — единственный путь на весь модуль.
///
/// И измерение (iced зовёт его через `Paragraph::with_text`, от результата
/// зависит layout), и отрисовка (`fill_text`) обязаны получать одинаково
/// разложенный буфер и одинаковые метрики: иначе виджет измеряется по одним
/// правилам, а рисуется по другим. Отсюда и один путь на оба: две обвязки
/// вокруг общего `shape_text` заводят каждая свой кэш, свой `min_x` и свою
/// формулу вертикального центрирования, и те расходятся.
fn build_paragraph(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    font_map: &HashMap<String, String>,
    default_family: &str,
    text: iced_core::Text<&str, Font>,
    origin: &str,
) -> RealParagraph {
    let font_family = resolve_family(font_system.db(), font_map, default_family, &text.font.family);

    let buffer = shape_text(
        font_system,
        text.content,
        text.size,
        text.line_height,
        text.bounds.width,
        &font_family,
        text.shaping,
        origin,
    );

    // Границы по горизонтали берутся из растровых образов глифов, а не из их
    // advance: у наклонного или свисающего глифа чернила выходят за перо, и
    // без этого текст обрезался бы по краю.
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut line_count = 0;

    for run in buffer.layout_runs() {
        line_count += 1;
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            if let Some(image) = swash_cache.get_image(font_system, physical.cache_key) {
                let left = glyph.x + image.placement.left as f32;
                min_x = min_x.min(left);
                max_x = max_x.max(left + image.placement.width as f32);
            } else {
                min_x = min_x.min(glyph.x);
                max_x = max_x.max(glyph.x + glyph.w);
            }
        }
    }

    if !min_x.is_finite() {
        min_x = 0.0;
        max_x = 0.0;
    }

    let visual_height = line_count as f32 * buffer.metrics().line_height;

    RealParagraph {
        buffer: Some(buffer),
        horizontal_alignment: text.horizontal_alignment,
        vertical_alignment: text.vertical_alignment,
        bounds: text.bounds,
        min_x,
        max_x,
        visual_height,
    }
}

impl iced_core::text::Paragraph for RealParagraph {
    type Font = Font;
    /// iced зовёт это статически, без доступа к рендереру, поэтому шрифтовое
    /// хозяйство приезжает сюда через `ScopeGuard` (thread-local). Вся работа —
    /// в `build_paragraph`, общем с путём отрисовки.
    fn with_text(text: iced_core::Text<&str, Self::Font>) -> Self {
        FONT_SYSTEM.with(|fs_cell| {
            SWASH_CACHE.with(|sc_cell| {
                FONT_MAP.with(|fm_cell| {
                    DEFAULT_FAMILY.with(|df_cell| {
                        match (*fs_cell.borrow_mut(), *sc_cell.borrow_mut(), *fm_cell.borrow(), *df_cell.borrow()) {
                            (Some(mut fs), Some(mut sc), Some(fm), Some(df)) => unsafe {
                                build_paragraph(fs.as_mut(), sc.as_mut(), fm.as_ref(), df.as_ref(), text, "with_text")
                            },
                            // Без ScopeGuard параграф не рендерится.
                            _ => Self::default(),
                        }
                    })
                })
            })
        })
    }
    fn with_spans<Link>(
        _: iced_core::Text<&[iced_core::text::Span<'_, Link, Self::Font>], Self::Font>,
    ) -> Self {
        Self::default()
    }
    fn resize(&mut self, size: Size) {
        self.bounds = size;
        if let Some(buffer) = &mut self.buffer {
            FONT_SYSTEM.with(|fs_cell| {
                if let Some(mut fs_ptr) = *fs_cell.borrow_mut() {
                    let font_system = unsafe { fs_ptr.as_mut() };
                    buffer.set_size(font_system, Some(size.width), Some(size.height));
                    buffer.shape_until_scroll(font_system, false);
                }
            });
        }
    }
    fn compare(&self, _: iced_core::Text<(), Self::Font>) -> iced_core::text::Difference {
        iced_core::text::Difference::None
    }
    fn horizontal_alignment(&self) -> iced_core::alignment::Horizontal {
        self.horizontal_alignment
    }
    fn vertical_alignment(&self) -> iced_core::alignment::Vertical {
        self.vertical_alignment
    }
    fn min_bounds(&self) -> Size {
        if self.buffer.is_some() {
            Size::new(self.max_x - self.min_x, self.visual_height)
        } else {
            Size::ZERO
        }
    }
    fn hit_span(&self, _: Point) -> Option<usize> {
        None
    }
    fn span_bounds(&self, _: usize) -> Vec<iced_core::Rectangle> {
        Vec::new()
    }
    fn hit_test(&self, _: Point) -> Option<iced_core::text::Hit> {
        None
    }
    fn grapheme_position(&self, _: usize, _: usize) -> Option<Point> {
        None
    }
}

#[derive(Default)]
pub struct DummyEditor;
impl iced_core::text::Editor for DummyEditor {
    type Font = Font;
    fn bounds(&self) -> Size {
        Size::ZERO
    }
    fn selection(&self) -> Option<String> {
        None
    }
    fn with_text(_: &str) -> Self {
        Self
    }
    fn is_empty(&self) -> bool {
        true
    }
    fn cursor(&self) -> iced_core::text::editor::Cursor {
        iced_core::text::editor::Cursor::Caret(Point::ORIGIN)
    }
    fn line(&self, _: usize) -> Option<&str> {
        None
    }
    fn line_count(&self) -> usize {
        0
    }
    fn perform(&mut self, _: iced_core::text::editor::Action) {}
    fn min_bounds(&self) -> Size {
        Size::ZERO
    }
    fn update(
        &mut self,
        _: Size,
        _: Self::Font,
        _: Pixels,
        _: LineHeight,
        _: iced_core::text::Wrapping,
        _: &mut impl Highlighter,
    ) {
    }
    fn highlight<H>(
        &mut self,
        _: Self::Font,
        _: &mut H,
        _: impl Fn(&H::Highlight) -> highlighter::Format<Self::Font>,
    ) where
        H: Highlighter,
    {
    }
    fn cursor_position(&self) -> (usize, usize) {
        (0, 0)
    }
}

impl iced_core::text::Renderer for GpuRenderer {
    type Font = Font;
    type Paragraph = RealParagraph;
    type Editor = DummyEditor;
    const ICON_FONT: Font = Font::DEFAULT;
    const CHECKMARK_ICON: char = ' ';
    const ARROW_DOWN_ICON: char = ' ';

    fn default_font(&self) -> Font {
        Font::DEFAULT
    }
    fn default_size(&self) -> Pixels {
        Pixels(16.0)
    }

    fn fill_paragraph(
        &mut self,
        p: &Self::Paragraph,
        pos: Point,
        color: Color,
        _clip: iced_core::Rectangle,
    ) {
        if let Some(buffer) = &p.buffer {
            let y_offset = match p.vertical_alignment {
                iced_core::alignment::Vertical::Center => (p.min_bounds().height) / 2.0,
                iced_core::alignment::Vertical::Bottom => p.min_bounds().height,
                _ => 0.0,
            };

            let adjusted_pos = Point::new(pos.x - p.min_x, pos.y - y_offset);
            self.prepare_glyphs(buffer);
            self.draw_buffer(buffer, adjusted_pos, color);
        }
    }

    fn fill_editor(&mut self, _e: &Self::Editor, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}

    /// Текст без параграфа: iced зовёт это из виджетов, которые не кэшируют
    /// разложенный текст сами (pick_list, checkbox, text_editor, меню оверлея).
    /// Ни один из них в наборе виджетов сервиса не встречается, но путь обязан
    /// быть согласован с основным — поэтому он просто раскладывает текст тем же
    /// `build_paragraph` и отдаёт его в `fill_paragraph`. Своего кэша здесь нет:
    /// он был бы вторым, ни с чем не сверяемым представлением того же текста.
    fn fill_text(
        &mut self,
        text: iced_core::Text,
        pos: Point,
        color: Color,
        clip: iced_core::Rectangle,
    ) {
        if text.content.is_empty() {
            return;
        }

        let paragraph = build_paragraph(
            &mut self.font_system,
            &mut self.swash_cache,
            &self.font_map,
            &self.default_family,
            iced_core::Text {
                content: text.content.as_str(),
                bounds: text.bounds,
                size: text.size,
                line_height: text.line_height,
                font: text.font,
                horizontal_alignment: text.horizontal_alignment,
                vertical_alignment: text.vertical_alignment,
                shaping: text.shaping,
                wrapping: text.wrapping,
            },
            "fill_text",
        );

        self.fill_paragraph(&paragraph, pos, color, clip);
    }
}

impl GpuRenderer {
    fn draw_buffer(&mut self, buffer: &Buffer, pos: Point, color: Color) {
        let text_color = [color.r, color.g, color.b, color.a];

        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical_glyph = glyph.physical((0.0, 0.0), self.current_sf);
                let cache_key = physical_glyph.cache_key;

                if let Some(info) = self.glyph_cache.get(&cache_key) {
                    let x = pos.x + glyph.x + (info.offset_x as f32 / self.current_sf);
                    let y = pos.y + run.line_y - (info.offset_y as f32 / self.current_sf);
                    self.add_quad(
                        [
                            x,
                            y,
                            info.width as f32 / self.current_sf,
                            info.height as f32 / self.current_sf,
                        ],
                        text_color,
                        info.uv,
                        0.0, 0.0, 0.0, [0.0, 0.0, 0.0, 0.0],
                    );
                }
            }
        }
    }

    fn prepare_glyphs(&mut self, buffer: &Buffer) {
        let mut atlas_modified = false;

        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical_glyph = glyph.physical((0.0, 0.0), self.current_sf);
                let cache_key = physical_glyph.cache_key;

                if !self.glyph_cache.contains_key(&cache_key) {
                    if let Some(image) = self.swash_cache.get_image(&mut self.font_system, cache_key)
                    {
                        let width = image.placement.width;
                        let height = image.placement.height;

                        if self.current_atlas_x + width + 2 > self.atlas_width {
                            self.current_atlas_x = 2;
                            self.current_atlas_y += self.row_height + 2;
                            self.row_height = 0;
                        }

                        if self.current_atlas_y + height + 2 > self.atlas_height {
                            self.current_atlas_x = 2;
                            self.current_atlas_y = 2;
                            self.row_height = 0;
                            self.glyph_cache.clear();
                        }

                        let x = self.current_atlas_x;
                        let y = self.current_atlas_y;

                        for r in 0..height {
                            for c in 0..width {
                                let src_idx = (r * width + c) as usize;
                                let dest_idx =
                                    (((y + r) * self.atlas_width + (x + c)) * 4) as usize;

                                if dest_idx + 4 <= self.atlas_data.len() {
                                    match image.content {
                                        cosmic_text::SwashContent::Mask => {
                                            if src_idx < image.data.len() {
                                                let val = image.data[src_idx];
                                                self.atlas_data[dest_idx] = 255;
                                                self.atlas_data[dest_idx + 1] = 255;
                                                self.atlas_data[dest_idx + 2] = 255;
                                                self.atlas_data[dest_idx + 3] = val;
                                            }
                                        }
                                        cosmic_text::SwashContent::Color => {
                                            if src_idx * 4 + 4 <= image.data.len() {
                                                self.atlas_data[dest_idx] = image.data[src_idx * 4];
                                                self.atlas_data[dest_idx + 1] =
                                                    image.data[src_idx * 4 + 1];
                                                self.atlas_data[dest_idx + 2] =
                                                    image.data[src_idx * 4 + 2];
                                                self.atlas_data[dest_idx + 3] =
                                                    image.data[src_idx * 4 + 3];
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                        let u1 = x as f32 / self.atlas_width as f32;
                        let v1 = y as f32 / self.atlas_height as f32;
                        let u2 = (x + width) as f32 / self.atlas_width as f32;
                        let v2 = (y + height) as f32 / self.atlas_height as f32;

                        self.glyph_cache.insert(
                            cache_key,
                            GlyphInfo {
                                uv: [u1, v1, u2, v2],
                                width,
                                height,
                                offset_x: image.placement.left,
                                offset_y: image.placement.top,
                            },
                        );

                        self.current_atlas_x += width + 2;
                        self.row_height = self.row_height.max(height);
                        self.atlas_dirty_y_min = self.atlas_dirty_y_min.min(y);
                        self.atlas_dirty_y_max = self.atlas_dirty_y_max.max(y + height);
                        atlas_modified = true;
                    }
                }
            }
        }

        if atlas_modified {
            }
    }
}
