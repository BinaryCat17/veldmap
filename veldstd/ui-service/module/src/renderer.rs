use veldsdk::rpc::wgpu::*;
use veldsdk::rpc::host::{call_service, gpu_write_resource};
use iced_core::{Transformation, Size, Theme, Point, Pixels, Font, Color};
use iced_core::text::{LineHeight, Highlighter, highlighter};
use iced_runtime::user_interface::UserInterface;
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping};
use std::collections::HashMap;
use prost::Message;
use crate::state::PluginUiState;
use veldsdk::wgpu::wgpu_proxy::WgpuRecorder;

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
    ExternalImage { bounds: iced_core::Rectangle, texture_id: u64 },
}

pub struct GpuRenderer {
    pub vertices: Vec<Vertex>,
    pub draw_commands: Vec<DrawCmd>,
    font_system: FontSystem,
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
}

impl GpuRenderer {
    pub fn new(_default_font_name: &str, fonts: Vec<(&str, &[u8])>) -> Self {
        let mut font_system = FontSystem::new();
        let mut font_map = HashMap::new();

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
            vertices: Vec::with_capacity(4096),
            draw_commands: Vec::new(),
            font_system,
            swash_cache: SwashCache::new(),
            atlas_texture_id: None,
            atlas_bind_group_id: None,
            bgl_id: None,
            glyph_cache: HashMap::new(),
            atlas_width: 1024,
            atlas_height: 1024,
            atlas_data: {
                let mut data = vec![0; 1024 * 1024 * 4];
                for i in 0..4 { data[i] = 255; }
                data
            },
            current_atlas_x: 2, 
            current_atlas_y: 0,
            row_height: 1,
            atlas_dirty: true,
            font_map,
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.draw_commands.clear();
    }

    pub fn add_quad(&mut self, rect: [f32; 4], color: [f32; 4], uv: [f32; 4]) {
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

    pub fn render_to_texture(&mut self, plugin: &PluginUiState, ui: &mut UserInterface<'_, crate::converter::UiMessage, Theme, GpuRenderer>, width: u32, height: u32, sf: f32, cursor: iced_core::mouse::Cursor) -> anyhow::Result<()> {
        let mut recorder = WgpuRecorder::new(width, height);
        
        let mut ui_pipeline = plugin.ui_pipeline.borrow_mut();
        if ui_pipeline.is_none() {
            let shader_source = include_str!("shaders.wgsl");
            let sh_req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateShader(CreateShaderModule {
                    source: shader_source.into(), label: "UI Shader".into()
                }))
            };
            let sh_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", sh_req.encode_to_vec())?[..])?;
            if let Some(sh) = sh_res.handle {
                let pip_req = GpuResourceRequest {
                    command: Some(gpu_resource_request::Command::CreatePipeline(CreateRenderPipeline {
                        shader_id: sh.id, label: "UI Pipeline".into(), 
                        vertex_entry: "vs_main".into(), fragment_entry: "fs_main".into(),
                        ..Default::default()
                    }))
                };
                let pip_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", pip_req.encode_to_vec())?[..])?;
                *ui_pipeline = pip_res.handle.map(|h| h.id);
            }
        }

        let mut uniform_buffer = plugin.uniform_buffer.borrow_mut();
        let mut uniform_buffer_id = plugin.uniform_buffer_id.borrow_mut();
        if uniform_buffer.is_none() {
            let buf_req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                    size: 16, usage: 64, mapped_at_creation: false
                }))
            };
            let buf_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", buf_req.encode_to_vec())?[..])?;
            if let Some(bh) = buf_res.handle {
                *uniform_buffer_id = Some(bh.id);
                let bgl_req = GpuResourceRequest {
                    command: Some(gpu_resource_request::Command::CreateBindGroupLayout(CreateBindGroupLayout {
                        label: "UI Uniform BGL".into(),
                        entries: vec![BindGroupLayoutEntry {
                            binding: 0, visibility: 3, ty: Some(bind_group_layout_entry::Ty::Buffer(BufferBindingLayout { r#type: 1, ..Default::default() }))
                        }]
                    }))
                };
                let bgl_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", bgl_req.encode_to_vec())?[..])?;
                if let Some(bgl) = bgl_res.handle {
                    let bg_req = GpuResourceRequest {
                        command: Some(gpu_resource_request::Command::CreateBindGroup(CreateBindGroup {
                            layout_id: bgl.id, entries: vec![BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::BufferId(bh.id)) }], label: "UI Uniform BG".into()
                        }))
                    };
                    *uniform_buffer = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", bg_req.encode_to_vec())?[..])?.handle;
                }
            }
        }

        if let Some(u_id) = *uniform_buffer_id {
            let res_data: [f32; 2] = [width as f32 / sf, height as f32 / sf];
            let data = unsafe { std::slice::from_raw_parts(res_data.as_ptr() as *const u8, 8) };
            let _ = gpu_write_resource(u_id, 0, data);
        }

        ui.draw(self, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
        self.ensure_resources();

        if !self.vertices.is_empty() || !self.draw_commands.is_empty() {
            let mut vertex_buffer = plugin.vertex_buffer.borrow_mut();
            if vertex_buffer.is_none() {
                let req = GpuResourceRequest {
                    command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                        size: 1024 * 1024 * 4, usage: 32, mapped_at_creation: false
                    }))
                };
                *vertex_buffer = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?.handle;
            }

            if let (Some(pipeline), Some(v_h), Some(u_h)) = (*ui_pipeline, &*vertex_buffer, &*uniform_buffer) {
                let data = unsafe { std::slice::from_raw_parts(self.vertices.as_ptr() as *const u8, self.vertices.len() * 32) };
                let _ = gpu_write_resource(v_h.id, 0, data);

                recorder.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

                let mut current_vertex_offset = 0;
                for cmd in &self.draw_commands {
                    match cmd {
                        DrawCmd::Quads { count } => {
                            recorder.set_pipeline(pipeline);
                            recorder.set_vertex_buffer(0, v_h.id, (current_vertex_offset * 32) as u64, (*count * 32) as u64);
                            recorder.set_bind_group(1, u_h.id);
                            if let Some(atlas_bg) = self.atlas_bind_group_id {
                                recorder.set_bind_group(0, atlas_bg);
                            }
                            recorder.draw(0..*count, 0..1);
                            current_vertex_offset += *count;
                        }
                        DrawCmd::ExternalImage { .. } => {}
                    }
                }
            }

            let mut ui_texture = plugin.ui_texture.borrow_mut();
            if ui_texture.is_none() {
                let req = GpuResourceRequest {
                    command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
                        width, height, format: 0, usage: 16 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1
                    }))
                };
                *ui_texture = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?.handle;
            }

            if let Some(ui_tex) = &*ui_texture {
                let _ = recorder.submit(ui_tex.id, Some(veldsdk::rpc::wgpu::GpuColor { r: 0.1, g: 0.1, b: 0.2, a: 1.0 }));
                let _ = veldsdk::app::AppBridge::display_frame(ui_tex.clone(), width, height);
            }
        }
        Ok(())
    }

    fn ensure_resources(&mut self) {
        if self.bgl_id.is_none() {
            let req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateBindGroupLayout(CreateBindGroupLayout {
                    label: "Iced Atlas BGL".into(),
                    entries: vec![
                        BindGroupLayoutEntry { binding: 0, visibility: 2, ty: Some(bind_group_layout_entry::Ty::Texture(TextureBindingLayout { sample_type: 1, view_dimension: 2, multisampled: false })) },
                        BindGroupLayoutEntry { binding: 1, visibility: 2, ty: Some(bind_group_layout_entry::Ty::Sampler(SamplerBindingLayout { r#type: 1 })) },
                    ],
                }))
            };
            if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
                if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                    self.bgl_id = res.handle.map(|h| h.id);
                }
            }
        }

        if self.atlas_texture_id.is_none() {
            let req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
                    width: self.atlas_width, height: self.atlas_height, format: 0, usage: 2 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1,
                }))
            };
            if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
                if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                    self.atlas_texture_id = res.handle.map(|h| h.id);
                    self.atlas_dirty = true;
                }
            }
        }
        
        if self.atlas_bind_group_id.is_none() && self.atlas_texture_id.is_some() && self.bgl_id.is_some() {
            let sampler_req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateSampler(CreateSampler { mag_filter: 1, min_filter: 1, ..Default::default() }))
            };
            let sampler_id = call_service("wgpu", "create_resource", sampler_req.encode_to_vec()).ok().and_then(|b| GpuResourceResponse::decode(&b[..]).ok()).and_then(|r| r.handle).map(|h| h.id).unwrap_or(0);
            let req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateBindGroup(CreateBindGroup {
                    layout_id: self.bgl_id.unwrap(), entries: vec![
                        BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::TextureViewId(self.atlas_texture_id.unwrap())) },
                        BindGroupEntry { binding: 1, resource: Some(bind_group_entry::Resource::SamplerId(sampler_id)) },
                    ],
                    label: "Iced Atlas BG".into(),
                }))
            };
            if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
                if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                    self.atlas_bind_group_id = res.handle.map(|h| h.id);
                }
            }
        }

        if self.atlas_dirty {
            if let Some(tid) = self.atlas_texture_id {
                let _ = gpu_write_resource(tid, 0, &self.atlas_data);
                self.atlas_dirty = false;
            }
        }
    }
}

impl iced_widget::core::renderer::Renderer for GpuRenderer {
    fn fill_quad(&mut self, quad: iced_widget::core::renderer::Quad, background: impl Into<iced_core::Background>) {
        let color = match background.into() {
            iced_core::Background::Color(c) => [c.r, c.g, c.b, c.a],
            _ => [1.0, 1.0, 1.0, 1.0],
        };
        self.add_quad([quad.bounds.x, quad.bounds.y, quad.bounds.width, quad.bounds.height], color, [0.0, 0.0, 0.0, 0.0]);
    }
    fn clear(&mut self) { self.vertices.clear(); self.draw_commands.clear(); }
    fn start_layer(&mut self, _bounds: iced_core::Rectangle) {}
    fn end_layer(&mut self) {}
    fn start_transformation(&mut self, _transformation: Transformation) {}
    fn end_transformation(&mut self) {}
}

impl iced_core::image::Renderer for GpuRenderer {
    type Handle = iced_core::image::Handle;
    fn measure_image(&self, _handle: &iced_core::image::Handle) -> Size<u32> { Size::new(100, 100) }
    fn draw_image(&mut self, _handle: iced_core::Image, _at: iced_core::Rectangle) {}
}

#[derive(Default)]
pub struct DummyParagraph;
impl iced_core::text::Paragraph for DummyParagraph {
    type Font = Font;
    fn with_text(_: iced_core::Text<&str, Self::Font>) -> Self { Self }
    fn with_spans<Link>(_: iced_core::Text<&[iced_core::text::Span<'_, Link, Self::Font>], Self::Font>) -> Self { Self }
    fn resize(&mut self, _: Size) {}
    fn compare(&self, _: iced_core::Text<(), Self::Font>) -> iced_core::text::Difference { iced_core::text::Difference::None }
    fn horizontal_alignment(&self) -> iced_core::alignment::Horizontal { iced_core::alignment::Horizontal::Left }
    fn vertical_alignment(&self) -> iced_core::alignment::Vertical { iced_core::alignment::Vertical::Top }
    fn min_bounds(&self) -> Size { Size::ZERO }
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
    type Paragraph = DummyParagraph;
    type Editor = DummyEditor;
    const ICON_FONT: Font = Font::DEFAULT;
    const CHECKMARK_ICON: char = ' ';
    const ARROW_DOWN_ICON: char = ' ';

    fn default_font(&self) -> Font { Font::DEFAULT }
    fn default_size(&self) -> Pixels { Pixels(16.0) }
    fn fill_paragraph(&mut self, _p: &Self::Paragraph, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}
    fn fill_editor(&mut self, _e: &Self::Editor, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}
    
    fn fill_text(&mut self, text: iced_core::Text, pos: Point, color: Color, _clip: iced_core::Rectangle) {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(text.size.0, text.line_height.to_absolute(text.size).0));
        let font_family = match &text.font.family {
            iced_core::font::Family::Name(name) => self.font_map.get(*name).map(|s| s.as_str()).unwrap_or("DejaVu Sans"),
            _ => "DejaVu Sans",
        };
        let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(font_family));
        buffer.set_text(&mut self.font_system, &text.content, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let text_color = [color.r, color.g, color.b, color.a];

        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical_glyph = glyph.physical((0.0, 0.0), 1.0);
                let cache_key = physical_glyph.cache_key;
                if !self.glyph_cache.contains_key(&cache_key) {
                    if let Some(image) = self.swash_cache.get_image(&mut self.font_system, cache_key) {
                        let width = image.placement.width;
                        let height = image.placement.height;
                        if self.current_atlas_x + width > self.atlas_width {
                            self.current_atlas_x = 2;
                            self.current_atlas_y += self.row_height + 2;
                            self.row_height = 0;
                        }
                        if self.current_atlas_y + height > self.atlas_height {
                            self.current_atlas_x = 2; self.current_atlas_y = 0; self.row_height = 0;
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
                    let x = pos.x + physical_glyph.x as f32 + info.offset_x as f32;
                    let y = pos.y + run.line_y as f32 + physical_glyph.y as f32 - info.offset_y as f32;
                    self.add_quad([x, y, info.width as f32, info.height as f32], text_color, info.uv);
                }
            }
        }
    }
}
