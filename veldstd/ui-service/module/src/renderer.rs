use iced_core::{Transformation, Size, Point, Pixels, Font, Color};
use iced_core::text::{LineHeight, Highlighter, highlighter, Paragraph};
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping};
use std::collections::HashMap;

use std::cell::RefCell;
use std::ptr::NonNull;

thread_local! {
    static FONT_SYSTEM: RefCell<Option<NonNull<FontSystem>>> = const { RefCell::new(None) };
    static SWASH_CACHE: RefCell<Option<NonNull<SwashCache>>> = const { RefCell::new(None) };
}

pub struct ScopeGuard;
impl ScopeGuard {
    pub fn new(fs: &mut FontSystem, sc: &mut SwashCache) -> Self {
        FONT_SYSTEM.with(|f| *f.borrow_mut() = Some(NonNull::from(fs)));
        SWASH_CACHE.with(|s| *s.borrow_mut() = Some(NonNull::from(sc)));
        Self
    }
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        FONT_SYSTEM.with(|f| *f.borrow_mut() = None);
        SWASH_CACHE.with(|s| *s.borrow_mut() = None);
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
    pub atlas_texture_id: Option<u64>,
    pub atlas_bind_group_id: Option<u64>,
    pub bgl_id: Option<u64>,
    glyph_cache: HashMap<cosmic_text::CacheKey, GlyphInfo>,
    atlas_data: Vec<u8>,
    atlas_width: u32,
    atlas_height: u32,
    current_atlas_x: u32,
    current_atlas_y: u32,
    row_height: u32,
    atlas_dirty: bool,
    atlas_dirty_y_min: u32,
    atlas_dirty_y_max: u32,
    font_map: HashMap<String, String>,
    text_cache: HashMap<String, Buffer>,
    pub current_sf: f32,
    scissor_stack: Vec<iced_core::Rectangle>,
    transformation_stack: Vec<Transformation>,
    pub current_width: u32,
    pub current_height: u32,
}

impl GpuRenderer {
    pub fn new(_default_font_name: &str, fonts: Vec<(&str, &[u8])>) -> Self {
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

        Self {
            vertices: Vec::with_capacity(8192),
            indices: Vec::with_capacity(12288),
            draw_commands: Vec::new(),
            font_system,
            swash_cache: SwashCache::new(),
            atlas_texture_id: None,
            atlas_bind_group_id: None,
            bgl_id: None,
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
            atlas_dirty: true,
            atlas_dirty_y_min: 0,
            atlas_dirty_y_max: 2048,
            font_map,
            text_cache: HashMap::new(),
            current_sf: 1.0,
            scissor_stack: Vec::new(),
            transformation_stack: vec![Transformation::IDENTITY],
            current_width: 0,
            current_height: 0,
        }
    }

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

    pub fn atlas_dirty_range(&self) -> (u64, &[u8]) {
        let y_min = self.atlas_dirty_y_min;
        let y_max = self.atlas_dirty_y_max.min(self.atlas_height);
        if y_min >= y_max { return (0, &[]); }

        let bytes_per_row = (self.atlas_width * 4) as u64;
        let start = (y_min as u64 * bytes_per_row) as usize;
        let end = (y_max as u64 * bytes_per_row) as usize;
        (start as u64, &self.atlas_data[start..end])
    }

    /// Returns full atlas data for dzn compatibility (full texture writes only)
    pub fn atlas_data_full(&self) -> &[u8] {
        &self.atlas_data
    }

    pub fn is_atlas_dirty(&self) -> bool {
        self.atlas_dirty_y_min < self.atlas_dirty_y_max
    }

    pub fn mark_atlas_clean(&mut self) {
        self.atlas_dirty = false;
        self.atlas_dirty_y_min = self.atlas_height;
        self.atlas_dirty_y_max = 0;
    }

    pub fn mark_atlas_dirty(&mut self) {
        self.atlas_dirty = true;
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
            .push(DrawCmd::ExternalImage {
                bounds,
                texture_id,
                index_count: 6
            });
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
        self.vertices.clear();
        self.indices.clear();
        self.draw_commands.clear();
        self.transformation_stack.clear();
        self.transformation_stack.push(Transformation::IDENTITY);
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

impl iced_core::text::Paragraph for RealParagraph {
    type Font = Font;
    fn with_text(text: iced_core::Text<&str, Self::Font>) -> Self {
        FONT_SYSTEM.with(|fs_cell| {
            if let Some(mut fs_ptr) = *fs_cell.borrow_mut() {
                let font_system = unsafe { fs_ptr.as_mut() };
                let mut size = text.size.0;
                let mut line_height = text.line_height.to_absolute(text.size).0;

                if size <= 0.0 {
                    veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "Cosmic-text: invalid font size: {}. Resetting to 16.0", size);
                    size = 16.0;
                }
                if line_height <= 0.0 {
                    veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "Cosmic-text: invalid line height: {}. Resetting to {}", line_height, size * 1.2);
                    line_height = size * 1.2;
                }

                let mut buffer = Buffer::new(
                    font_system,
                    Metrics::new(size, line_height),
                );

                if text.bounds.width < f32::INFINITY {
                    buffer.set_size(font_system, Some(text.bounds.width), None);
                }

                let font_family = match &text.font.family {
                    iced_core::font::Family::Name(name) => name,
                    _ => "JetBrains Mono",
                };
                let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(font_family));
                buffer.set_text(font_system, text.content, attrs, Shaping::Advanced);
                buffer.shape_until_scroll(font_system, false);

                let mut min_x = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut line_count = 0;

                SWASH_CACHE.with(|sc_cell| {
                    if let Some(mut sc_ptr) = *sc_cell.borrow_mut() {
                        let swash_cache = unsafe { sc_ptr.as_mut() };
                        for run in buffer.layout_runs() {
                            line_count += 1;
                            for glyph in run.glyphs {
                                let physical = glyph.physical((0.0, 0.0), 1.0);
                                if let Some(image) =
                                    swash_cache.get_image(font_system, physical.cache_key)
                                {
                                    let left = glyph.x + image.placement.left as f32;
                                    let right = left + image.placement.width as f32;
                                    min_x = min_x.min(left);
                                    max_x = max_x.max(right);
                                } else {
                                    min_x = min_x.min(glyph.x);
                                    max_x = max_x.max(glyph.x + glyph.w);
                                }
                            }
                        }
                    }
                });

                if !min_x.is_finite() {
                    min_x = 0.0;
                    max_x = 0.0;
                }

                let visual_height = line_count as f32 * buffer.metrics().line_height;

                Self {
                    buffer: Some(buffer),
                    horizontal_alignment: text.horizontal_alignment,
                    vertical_alignment: text.vertical_alignment,
                    bounds: text.bounds,
                    min_x,
                    max_x,
                    visual_height,
                }
            } else {
                Self::default()
            }
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

    fn fill_text(
        &mut self,
        text: iced_core::Text,
        pos: Point,
        color: Color,
        _clip: iced_core::Rectangle,
    ) {
        if text.content.is_empty() {
            return;
        }

        let font_family = match &text.font.family {
            iced_core::font::Family::Name(name) => self
                .font_map
                .get(*name)
                .map(|s| s.as_str())
                .unwrap_or("JetBrains Mono"),
            _ => "JetBrains Mono",
        };

        let cache_key = format!(
            "{}_{}_{:?}_{:?}_{}",
            text.content, text.size.0, font_family, text.shaping, text.bounds.width
        );

        if !self.text_cache.contains_key(&cache_key) {
            if self.text_cache.len() > 1000 {
                self.text_cache.clear();
            }

            let mut size = text.size.0;
            let mut line_height = text.line_height.to_absolute(text.size).0;

            if size <= 0.0 {
                veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "Cosmic-text fill_text: invalid font size: {}. Resetting to 16.0", size);
                size = 16.0;
            }
            if line_height <= 0.0 {
                veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "Cosmic-text fill_text: invalid line height: {}. Resetting to {}", line_height, size * 1.2);
                line_height = size * 1.2;
            }

            let mut buffer = Buffer::new(
                &mut self.font_system,
                Metrics::new(size, line_height),
            );

            if text.bounds.width.is_finite() {
                buffer.set_size(&mut self.font_system, Some(text.bounds.width), None);
            }

            let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(font_family));

            let shaping_type = match text.shaping {
                iced_core::text::Shaping::Basic => Shaping::Basic,
                iced_core::text::Shaping::Advanced => Shaping::Advanced,
            };

            buffer.set_text(&mut self.font_system, &text.content, attrs, shaping_type);
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.text_cache.insert(cache_key.clone(), buffer);
        }

        let buffer = self.text_cache.remove(&cache_key).unwrap();

        let mut min_x = f32::INFINITY;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                if let Some(image) = self.swash_cache.get_image(&mut self.font_system, physical.cache_key) {
                    let left = glyph.x + image.placement.left as f32;
                    min_x = min_x.min(left);
                } else {
                    min_x = min_x.min(glyph.x);
                }
            }
        }

        if !min_x.is_finite() {
            min_x = 0.0;
        }

        let y_offset = match text.vertical_alignment {
            iced_core::alignment::Vertical::Center => text.bounds.height / 2.0,
            iced_core::alignment::Vertical::Bottom => text.bounds.height,
            _ => 0.0,
        };

        let adjusted_pos = Point::new(pos.x - min_x, pos.y - y_offset);

        self.prepare_glyphs(&buffer);
        self.draw_buffer(&buffer, adjusted_pos, color);
        
        self.text_cache.insert(cache_key, buffer);
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
            self.atlas_dirty = true;
        }
    }
}
