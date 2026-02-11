use iced_core::{Transformation, Size, Point, Pixels, Font, Color};
use iced_core::text::{LineHeight, Highlighter, highlighter};
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping};
use std::collections::HashMap;

use lazy_static::lazy_static;
use std::sync::Mutex;

lazy_static! {
    static ref FONT_SYSTEM: Mutex<FontSystem> = Mutex::new(FontSystem::new());
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex { pub pos: [f32; 2], pub color: [f32; 4], pub uv: [f32; 2] }

#[derive(Clone, Copy)]
struct GlyphInfo {
    uv: [f32; 4],
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
}

pub enum DrawCmd {
    Quads { count: u32 },
    Scissor { x: u32, y: u32, width: u32, height: u32 },
    ExternalImage { bounds: iced_core::Rectangle, texture_id: u64 },
}

pub struct GpuRenderer {
    pub vertices: Vec<Vertex>,
    pub draw_commands: Vec<DrawCmd>,
    swash_cache: SwashCache,
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
    font_map: HashMap<String, String>,
    pub current_sf: f32,
    scissor_stack: Vec<iced_core::Rectangle>,
    transformation_stack: Vec<Transformation>,
    pub current_width: u32,
    pub current_height: u32,
}

impl GpuRenderer {
    pub fn new(_default_font_name: &str, fonts: Vec<(&str, &[u8])>) -> Self {
        let mut font_map = HashMap::new();

        {
            let mut font_system = FONT_SYSTEM.lock().unwrap();
            for (name, data) in fonts {
                let source = cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(data.to_vec()));
                let ids = font_system.db_mut().load_font_source(source);
                if let Some(first_id) = ids.first() {
                    if let Some(info) = font_system.db().face(*first_id) {
                         font_map.insert(name.to_string(), info.families[0].0.clone());
                    }
                }
            }
        }

        Self { 
            vertices: Vec::with_capacity(8192),
            draw_commands: Vec::new(),
            swash_cache: SwashCache::new(),
            atlas_texture_id: None,
            atlas_bind_group_id: None,
            bgl_id: None,
            glyph_cache: HashMap::new(),
            atlas_width: 2048,
            atlas_height: 2048,
            atlas_data: {
                let mut data = vec![0; 2048 * 2048 * 4];
                // Заполняем область 4x4 белым цветом для сплошных заливок
                for y in 0..4 {
                    for x in 0..4 {
                        let idx = (y * 2048 + x) * 4;
                        for i in 0..4 { data[idx + i] = 255; }
                    }
                }
                data
            },
            current_atlas_x: 6, 
            current_atlas_y: 6,
            row_height: 1,
            atlas_dirty: true,
            font_map,
            current_sf: 1.0,
            scissor_stack: Vec::new(),
            transformation_stack: vec![Transformation::IDENTITY],
            current_width: 0,
            current_height: 0,
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
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

    pub fn atlas_data(&self) -> (u32, u32, &[u8]) {
        (self.atlas_width, self.atlas_height, &self.atlas_data)
    }

    pub fn is_atlas_dirty(&self) -> bool {
        self.atlas_dirty
    }

    pub fn mark_atlas_clean(&mut self) {
        self.atlas_dirty = false;
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

    pub fn add_quad(&mut self, rect: [f32; 4], color: [f32; 4], uv: [f32; 4]) {
        let rect = self.transform_rect(rect);
        let x = rect[0]; let y = rect[1]; let w = rect[2]; let h = rect[3];
        let u1 = uv[0]; let v1 = uv[1]; let u2 = uv[2]; let v2 = uv[3];
        
        self.vertices.push(Vertex { pos: [x, y], color, uv: [u1, v1] });
        self.vertices.push(Vertex { pos: [x + w, y], color, uv: [u2, v1] });
        self.vertices.push(Vertex { pos: [x + w, y + h], color, uv: [u2, v2] });
        self.vertices.push(Vertex { pos: [x, y], color, uv: [u1, v1] });
        self.vertices.push(Vertex { pos: [x + w, y + h], color, uv: [u2, v2] });
        self.vertices.push(Vertex { pos: [x, y + h], color, uv: [u1, v2] });

        match self.draw_commands.last_mut() {
            Some(DrawCmd::Quads { count }) => *count += 6,
            _ => self.draw_commands.push(DrawCmd::Quads { count: 6 }),
        }
    }

    pub fn draw_wgpu_image(&mut self, bounds: iced_core::Rectangle, texture_id: u64) {
        self.draw_commands.push(DrawCmd::ExternalImage { bounds, texture_id });
    }
}

impl iced_widget::core::renderer::Renderer for GpuRenderer {
    fn fill_quad(&mut self, quad: iced_widget::core::renderer::Quad, background: impl Into<iced_core::Background>) {
        let color = match background.into() {
            iced_core::Background::Color(c) => [c.r, c.g, c.b, c.a],
            _ => [1.0, 1.0, 1.0, 1.0],
        };
        
        log::trace!("[UI-RENDERER] fill_quad: bounds={:?}, color={:?}", quad.bounds, color);
        
        // Используем UV, указывающий на центр нашей белой области 4x4 в начале атласа.
        let white_uv = [2.0/2048.0, 2.0/2048.0, 2.0/2048.0, 2.0/2048.0];
        
        // Отрисовка основного прямоугольника
        self.add_quad([quad.bounds.x, quad.bounds.y, quad.bounds.width, quad.bounds.height], color, white_uv);
        
        // Отрисовка границ (если есть)
        if quad.border.width > 0.0 {
            let bc = quad.border.color;
            let border_color = [bc.r, bc.g, bc.b, bc.a];
            let bw = quad.border.width;
            
            self.add_quad([quad.bounds.x, quad.bounds.y, quad.bounds.width, bw], border_color, white_uv);
            self.add_quad([quad.bounds.x, quad.bounds.y + quad.bounds.height - bw, quad.bounds.width, bw], border_color, white_uv);
            self.add_quad([quad.bounds.x, quad.bounds.y, bw, quad.bounds.height], border_color, white_uv);
            self.add_quad([quad.bounds.x + quad.bounds.width - bw, quad.bounds.y, bw, quad.bounds.height], border_color, white_uv);
        }
    }
    fn clear(&mut self) { 
        self.vertices.clear(); 
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
        let current = self.transformation_stack.last().cloned().unwrap_or(Transformation::IDENTITY);
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
            self.draw_commands.push(DrawCmd::Scissor { x, y, width: w, height: h });
        } else {
            self.draw_commands.push(DrawCmd::Scissor { x: 0, y: 0, width: self.current_width, height: self.current_height });
        }
    }
}

impl iced_core::image::Renderer for GpuRenderer {
    type Handle = iced_core::image::Handle;
    fn measure_image(&self, _handle: &iced_core::image::Handle) -> Size<u32> { Size::new(100, 100) }
    fn draw_image(&mut self, _handle: iced_core::Image, _at: iced_core::Rectangle) {}
}

pub struct RealParagraph {
    pub buffer: Option<Buffer>,
    pub horizontal_alignment: iced_core::alignment::Horizontal,
    pub vertical_alignment: iced_core::alignment::Vertical,
    pub bounds: Size,
}

impl Default for RealParagraph {
    fn default() -> Self {
        Self {
            buffer: None,
            horizontal_alignment: iced_core::alignment::Horizontal::Left,
            vertical_alignment: iced_core::alignment::Vertical::Top,
            bounds: Size::INFINITY,
        }
    }
}

impl iced_core::text::Paragraph for RealParagraph {
    type Font = Font;
    fn with_text(text: iced_core::Text<&str, Self::Font>) -> Self {
        let mut font_system = FONT_SYSTEM.lock().unwrap();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(text.size.0, text.line_height.to_absolute(text.size).0));
        
        if text.bounds.width < f32::INFINITY {
            buffer.set_size(&mut font_system, Some(text.bounds.width), None);
        }

        let font_family = match &text.font.family {
            iced_core::font::Family::Name(name) => name,
            _ => "DejaVu Sans",
        };
        let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(font_family));
        buffer.set_text(&mut font_system, text.content, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut font_system, false);
        
        let mut width: f32 = 0.0;
        for run in buffer.layout_runs() { width = width.max(run.line_w); }

        Self { 
            buffer: Some(buffer),
            horizontal_alignment: text.horizontal_alignment,
            vertical_alignment: text.vertical_alignment,
            bounds: text.bounds,
        } 
    }
    fn with_spans<Link>(_: iced_core::Text<&[iced_core::text::Span<'_, Link, Self::Font>], Self::Font>) -> Self { Self::default() }
    fn resize(&mut self, size: Size) {
        self.bounds = size;
        if let Some(buffer) = &mut self.buffer {
            let mut font_system = FONT_SYSTEM.lock().unwrap();
            buffer.set_size(&mut font_system, Some(size.width), Some(size.height));
            buffer.shape_until_scroll(&mut font_system, false);
        }
    }
    fn compare(&self, _: iced_core::Text<(), Self::Font>) -> iced_core::text::Difference { iced_core::text::Difference::None }
    fn horizontal_alignment(&self) -> iced_core::alignment::Horizontal { self.horizontal_alignment }
    fn vertical_alignment(&self) -> iced_core::alignment::Vertical { self.vertical_alignment }
    fn min_bounds(&self) -> Size { 
        if let Some(buf) = &self.buffer {
            let mut width: f32 = 0.0;
            for run in buf.layout_runs() { width = width.max(run.line_w); }
            let height = buf.layout_runs().count() as f32 * buf.metrics().line_height;
            Size::new(width, height)
        } else {
            Size::ZERO 
        }
    }
    fn hit_span(&self, _: Point) -> Option<usize> { None }
    fn span_bounds(&self, _: usize) -> Vec<iced_core::Rectangle> { Vec::new() }
    fn hit_test(&self, _: Point) -> Option<iced_core::text::Hit> { None }
    fn grapheme_position(&self, _: usize, _: usize) -> Option<Point> { None }
}

#[derive(Default)]
pub struct DummyEditor;
impl iced_core::text::Editor for DummyEditor {
    type Font = Font;
    fn bounds(&self) -> Size { Size::ZERO }
    fn selection(&self) -> Option<String> { None }
    fn with_text(_: &str) -> Self { Self }
    fn is_empty(&self) -> bool { true }
    fn cursor(&self) -> iced_core::text::editor::Cursor { iced_core::text::editor::Cursor::Caret(Point::ORIGIN) }
    fn line(&self, _: usize) -> Option<&str> { None }
    fn line_count(&self) -> usize { 0 }
    fn perform(&mut self, _: iced_core::text::editor::Action) {}
    fn min_bounds(&self) -> Size { Size::ZERO }
    fn update(&mut self, _: Size, _: Self::Font, _: Pixels, _: LineHeight, _: iced_core::text::Wrapping, _: &mut impl Highlighter) {}
    fn highlight<H>(&mut self, _: Self::Font, _: &mut H, _: impl Fn(&H::Highlight) -> highlighter::Format<Self::Font>) where H: Highlighter {}
    fn cursor_position(&self) -> (usize, usize) { (0, 0) }
}

impl iced_core::text::Renderer for GpuRenderer {
    type Font = Font;
    type Paragraph = RealParagraph;
    type Editor = DummyEditor;
    const ICON_FONT: Font = Font::DEFAULT;
    const CHECKMARK_ICON: char = ' ';
    const ARROW_DOWN_ICON: char = ' ';

    fn default_font(&self) -> Font { Font::DEFAULT }
    fn default_size(&self) -> Pixels { Pixels(16.0) }

    fn fill_paragraph(&mut self, p: &Self::Paragraph, pos: Point, color: Color, _clip: iced_core::Rectangle) {
        if let Some(buffer) = &p.buffer {
            let mut width: f32 = 0.0;
            for run in buffer.layout_runs() { width = width.max(run.line_w); }
            let height = buffer.layout_runs().count() as f32 * buffer.metrics().line_height;
            
            let x_offset = match p.horizontal_alignment {
                iced_core::alignment::Horizontal::Center => width / 2.0,
                iced_core::alignment::Horizontal::Right => width,
                _ => 0.0,
            };
             let y_offset = match p.vertical_alignment {
                iced_core::alignment::Vertical::Center => height / 2.0,
                iced_core::alignment::Vertical::Bottom => height,
                _ => 0.0,
            };
            
            let adjusted_pos = Point::new(pos.x - x_offset, pos.y - y_offset);
            self.draw_buffer(buffer, adjusted_pos, color);
        }
    }

    fn fill_editor(&mut self, _e: &Self::Editor, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}
    
    fn fill_text(&mut self, text: iced_core::Text, pos: Point, color: Color, _clip: iced_core::Rectangle) {
        if text.content.is_empty() { return; }
        let mut font_system = FONT_SYSTEM.lock().unwrap();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(text.size.0, text.line_height.to_absolute(text.size).0));
        
        if text.bounds.width.is_finite() {
            buffer.set_size(&mut font_system, Some(text.bounds.width), None);
        }

        let font_family = match &text.font.family {
            iced_core::font::Family::Name(name) => self.font_map.get(*name).map(|s| s.as_str()).unwrap_or("DejaVu Sans"),
            _ => "DejaVu Sans",
        };
        let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(font_family));
        
        let shaping_type = match text.shaping {
            iced_core::text::Shaping::Basic => Shaping::Basic,
            iced_core::text::Shaping::Advanced => Shaping::Advanced,
        };

        buffer.set_text(&mut font_system, &text.content, attrs, shaping_type);
        buffer.shape_until_scroll(&mut font_system, false);
        
        // Отпускаем лок перед отрисовкой
        drop(font_system);

        let x_offset = match text.horizontal_alignment {
            iced_core::alignment::Horizontal::Center => text.bounds.width / 2.0,
            iced_core::alignment::Horizontal::Right => text.bounds.width,
            _ => 0.0,
        };
        let y_offset = match text.vertical_alignment {
            iced_core::alignment::Vertical::Center => text.bounds.height / 2.0,
            iced_core::alignment::Vertical::Bottom => text.bounds.height,
            _ => 0.0,
        };
        
        let adjusted_pos = Point::new(pos.x - x_offset, pos.y - y_offset);
        
        self.draw_buffer(&buffer, adjusted_pos, color);
    }
}

impl GpuRenderer {
    fn draw_buffer(&mut self, buffer: &Buffer, pos: Point, color: Color) {
        let text_color = [color.r, color.g, color.b, color.a];
        let mut font_system = FONT_SYSTEM.lock().unwrap();
        
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical_glyph = glyph.physical((0.0, 0.0), self.current_sf);
                let cache_key = physical_glyph.cache_key;
                if !self.glyph_cache.contains_key(&cache_key) {
                    if let Some(image) = self.swash_cache.get_image(&mut font_system, cache_key) {
                        let width = image.placement.width;
                        let height = image.placement.height;
                        if self.current_atlas_x + width + 2 > self.atlas_width {
                            self.current_atlas_x = 2;
                            self.current_atlas_y += self.row_height + 2;
                            self.row_height = 0;
                        }
                        if self.current_atlas_y + height + 2 > self.atlas_height {
                            self.current_atlas_x = 2; self.current_atlas_y = 2; self.row_height = 0;
                            self.glyph_cache.clear();
                        }
                        let x = self.current_atlas_x; let y = self.current_atlas_y;
                        for r in 0..height {
                            for c in 0..width {
                                let src_idx = (r * width + c) as usize;
                                let dest_idx = (((y + r) * self.atlas_width + (x + c)) * 4) as usize;
                                if dest_idx + 4 <= self.atlas_data.len() {
                                    match image.content {
                                        cosmic_text::SwashContent::Mask => {
                                            if src_idx < image.data.len() {
                                                let val = image.data[src_idx];
                                                self.atlas_data[dest_idx] = 255; self.atlas_data[dest_idx+1] = 255; self.atlas_data[dest_idx+2] = 255; self.atlas_data[dest_idx+3] = val;
                                            }
                                        }
                                        cosmic_text::SwashContent::Color => {
                                            if src_idx * 4 + 4 <= image.data.len() {
                                                self.atlas_data[dest_idx] = image.data[src_idx*4]; self.atlas_data[dest_idx+1] = image.data[src_idx*4+1]; self.atlas_data[dest_idx+2] = image.data[src_idx*4+2]; self.atlas_data[dest_idx+3] = image.data[src_idx*4+3];
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        self.atlas_dirty = true;
                        let u1 = x as f32 / self.atlas_width as f32; let v1 = y as f32 / self.atlas_height as f32;
                        let u2 = (x + width) as f32 / self.atlas_width as f32; let v2 = (y + height) as f32 / self.atlas_height as f32;
                        self.glyph_cache.insert(cache_key, GlyphInfo { uv: [u1, v1, u2, v2], width, height, offset_x: image.placement.left, offset_y: image.placement.top });
                        self.current_atlas_x += width + 2; self.row_height = self.row_height.max(height);
                    }
                }
                if let Some(info) = self.glyph_cache.get(&cache_key) {
                    let x = pos.x + glyph.x + (info.offset_x as f32 / self.current_sf);
                    let y = pos.y + run.line_y - (info.offset_y as f32 / self.current_sf);
                    self.add_quad([x, y, info.width as f32 / self.current_sf, info.height as f32 / self.current_sf], text_color, info.uv);
                }
            }
        }
    }
}