use crate::graphics::wgpu_proxy::WgpuRecorder;
use crate::iced::RawIcedRuntime;
use crate::core::{Command, BoxedFuture};
use crate::rpc::ui::UiEvent;
use iced_core::{Transformation, alignment, Size, Theme, Point, Pixels, Font, Event, Color};
use iced_core::text::{LineHeight, Highlighter, highlighter};
use iced_graphics::Viewport;
use iced_runtime::{UserInterface, user_interface};
use std::cell::RefCell;
use std::task::{Context, Poll};
use futures_util::task::noop_waker_ref;
use prost::Message;
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping};
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex { pos: [f32; 2], color: [f32; 4], uv: [f32; 2] }

#[derive(Clone, Copy)]
struct GlyphInfo {
    uv: [f32; 4], // u1, v1, u2, v2
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
}

/// A pure GPU renderer for Iced that builds vertex buffers.
pub struct GpuRenderer {
    vertices: Vec<Vertex>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    pub atlas_texture_id: Option<u64>,
    glyph_cache: HashMap<cosmic_text::CacheKey, GlyphInfo>,
    atlas_data: Vec<u8>,
    atlas_width: u32,
    atlas_height: u32,
    current_atlas_x: u32,
    current_atlas_y: u32,
    row_height: u32,
    atlas_dirty: bool,
}

impl GpuRenderer {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new();
        let font_data = include_bytes!("../../../veldgis/assets/DejaVuSans.ttf");
        let source = cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(font_data.as_slice()));
        font_system.db_mut().load_font_source(source);

        Self { 
            vertices: Vec::with_capacity(2048),
            font_system,
            swash_cache: SwashCache::new(),
            atlas_texture_id: None,
            glyph_cache: HashMap::new(),
            atlas_width: 1024,
            atlas_height: 1024,
            atlas_data: {
                let mut data = vec![0; 1024 * 1024];
                data[0] = 255; // White pixel at (0,0) for solid colors
                data
            },
            current_atlas_x: 2, // Start after white pixel + margin
            current_atlas_y: 0,
            row_height: 1,
            atlas_dirty: true,
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    fn ensure_atlas_texture(&mut self) {
        if self.atlas_texture_id.is_none() {
            use crate::rpc::services::{GpuResourceRequest, GpuResourceResponse};
            use crate::rpc::wgpu::CreateTexture;
            let req = GpuResourceRequest {
                command: Some(crate::rpc::services::gpu_resource_request::Command::CreateTexture(CreateTexture {
                    width: self.atlas_width,
                    height: self.atlas_height,
                    format: 9, // R8Unorm
                    usage: 2 | 4, // CopyDst | TextureBinding
                    dimension: 1, 
                    mip_level_count: 1,
                    sample_count: 1,
                    depth_or_array_layers: 1,
                }))
            };
            if let Ok(res_bytes) = crate::rpc::host::call_service("system", "create_resource", req.encode_to_vec()) {
                if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                    self.atlas_texture_id = res.handle.map(|h| h.id);
                    self.atlas_dirty = true;
                }
            }
        }
        
        if self.atlas_dirty {
            if let Some(tid) = self.atlas_texture_id {
                let _ = crate::rpc::host::gpu_write_resource(tid, 0, &self.atlas_data);
                self.atlas_dirty = false;
            }
        }
    }
}

fn add_quad(vertices: &mut Vec<Vertex>, rect: [f32; 4], color: [f32; 4], uv: [f32; 4]) {
    let x = rect[0]; let y = rect[1]; let w = rect[2]; let h = rect[3];
    let u1 = uv[0]; let v1 = uv[1]; let u2 = uv[2]; let v2 = uv[3];
    vertices.push(Vertex { pos: [x, y], color, uv: [u1, v1] });
    vertices.push(Vertex { pos: [x + w, y], color, uv: [u2, v1] });
    vertices.push(Vertex { pos: [x + w, y + h], color, uv: [u2, v2] });
    vertices.push(Vertex { pos: [x, y], color, uv: [u1, v1] });
    vertices.push(Vertex { pos: [x + w, y + h], color, uv: [u2, v2] });
    vertices.push(Vertex { pos: [x, y + h], color, uv: [u1, v2] });
}

impl iced_widget::core::renderer::Renderer for GpuRenderer {
    fn fill_quad(&mut self, quad: iced_widget::core::renderer::Quad, background: impl Into<iced_core::Background>) {
        let color = match background.into() {
            iced_core::Background::Color(c) => [c.r, c.g, c.b, c.a],
            _ => [1.0, 1.0, 1.0, 1.0],
        };
        // UV [0,0,0,0] maps to the white pixel at (0,0) in our atlas
        add_quad(&mut self.vertices, [quad.bounds.x, quad.bounds.y, quad.bounds.width, quad.bounds.height], color, [0.0, 0.0, 0.0, 0.0]);
    }

    fn start_layer(&mut self, _bounds: iced_core::Rectangle) {}
    fn end_layer(&mut self) {}
    fn start_transformation(&mut self, _transformation: Transformation) {}
    fn end_transformation(&mut self) {}
    fn reset(&mut self, _bounds: iced_core::Rectangle) {}
    fn allocate_image(&mut self, _handle: &iced_core::image::Handle, _cb: impl FnOnce(Result<iced_core::image::Allocation, iced_core::image::Error>) + Send + 'static) {}
}

#[derive(Default)]
pub struct DummyParagraph;
impl iced_core::text::Paragraph for DummyParagraph {
    type Font = Font;
    fn font(&self) -> Self::Font { Font::DEFAULT }
    fn size(&self) -> Pixels { Pixels(16.0) }
    fn line_height(&self) -> iced_core::text::LineHeight { iced_core::text::LineHeight::default() }
    fn shaping(&self) -> iced_core::text::Shaping { iced_core::text::Shaping::Basic }
    fn bounds(&self) -> Size { Size::ZERO }
    fn min_width(&self) -> f32 { 0.0 }
    fn hit_test(&self, _point: Point) -> Option<iced_core::text::Hit> { None }
    fn grapheme_position(&self, _line: usize, _index: usize) -> Option<Point> { None }
    fn with_text(_: iced_core::Text<&str, Self::Font>) -> Self { Self }
    fn with_spans<Link>(_: iced_core::Text<&[iced_core::text::Span<'_, Link, Self::Font>], Self::Font>) -> Self { Self }
    fn resize(&mut self, _: Size) {}
    fn compare(&self, _: iced_core::Text<(), Self::Font>) -> iced_core::text::Difference { iced_core::text::Difference::None }
    fn align_x(&self) -> iced_core::text::Alignment { iced_core::text::Alignment::Left }
    fn align_y(&self) -> alignment::Vertical { alignment::Vertical::Top }
    fn wrapping(&self) -> iced_core::text::Wrapping { iced_core::text::Wrapping::None }
    fn min_bounds(&self) -> Size { Size::ZERO }
    fn hit_span(&self, _: Point) -> Option<usize> { None }
    fn span_bounds(&self, _: usize) -> Vec<iced_core::Rectangle> { Vec::new() }
}

#[derive(Default)]
pub struct DummyEditor;
impl iced_core::text::Editor for DummyEditor {
    type Font = Font;
    fn bounds(&self) -> Size { Size::ZERO }
    fn selection(&self) -> iced_widget::text_editor::Selection { unsafe { std::mem::zeroed() } }
    fn with_text(_: &str) -> Self { Self }
    fn is_empty(&self) -> bool { true }
    fn cursor(&self) -> iced_widget::text_editor::Cursor { unsafe { std::mem::zeroed() } }
    fn line(&self, _: usize) -> Option<iced_widget::text_editor::Line<'_>> { None }
    fn line_count(&self) -> usize { 0 }
    fn perform(&mut self, _: iced_widget::text_editor::Action) {}
    fn move_to(&mut self, _: iced_widget::text_editor::Cursor) {}
    fn min_bounds(&self) -> Size { Size::ZERO }
    fn update(&mut self, _: Size, _: Self::Font, _: Pixels, _: LineHeight, _: iced_core::text::Wrapping, _: &mut impl Highlighter) {}
    fn highlight<H>(&mut self, _: Self::Font, _: &mut H, _: impl Fn(&H::Highlight) -> highlighter::Format<Self::Font>) where H: Highlighter {}
    fn copy(&self) -> Option<String> { None }
}

impl iced_core::image::Renderer for GpuRenderer {
    type Handle = iced_core::image::Handle;
    fn measure_image(&self, _handle: &iced_core::image::Handle) -> Option<Size<u32>> { Some(Size::new(100, 100)) }
    fn draw_image(&mut self, _handle: iced_core::Image, _at: iced_core::Rectangle, _bounds: iced_core::Rectangle) {}
    fn load_image(&self, _handle: &Self::Handle) -> Result<iced_core::image::Allocation, iced_core::image::Error> { todo!() }
}

impl iced_core::text::Renderer for GpuRenderer {
    type Font = Font;
    type Paragraph = DummyParagraph;
    type Editor = DummyEditor;
    const ICON_FONT: Font = Font::DEFAULT;
    const CHECKMARK_ICON: char = ' ';
    const ARROW_DOWN_ICON: char = ' ';
    const SCROLL_UP_ICON: char = ' ';
    const SCROLL_DOWN_ICON: char = ' ';
    const SCROLL_LEFT_ICON: char = ' ';
    const SCROLL_RIGHT_ICON: char = ' ';
    const ICED_LOGO: char = ' ';

    fn default_font(&self) -> Font { Font::DEFAULT }
    fn default_size(&self) -> Pixels { Pixels(16.0) }
    fn fill_paragraph(&mut self, _p: &Self::Paragraph, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}
    fn fill_editor(&mut self, _e: &Self::Editor, _pos: Point, _color: Color, _clip: iced_core::Rectangle) {}
    
    fn fill_text(&mut self, text: iced_core::Text, pos: Point, color: Color, _clip: iced_core::Rectangle) {
        self.ensure_atlas_texture();
        
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(text.size.0, text.line_height.to_absolute(text.size).0));
        buffer.set_text(&mut self.font_system, &text.content, cosmic_text::Attrs::new(), Shaping::Advanced);
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
                            self.current_atlas_x = 2;
                            self.current_atlas_y = 0;
                            self.row_height = 0;
                            self.glyph_cache.clear();
                        }

                        let x = self.current_atlas_x;
                        let y = self.current_atlas_y;

                        for r in 0..height {
                            let src_start = (r * width) as usize;
                            let dest_start = ((y + r) * self.atlas_width + x) as usize;
                            if src_start + width as usize <= image.data.len() && dest_start + width as usize <= self.atlas_data.len() {
                                let src_slice = &image.data[src_start..(src_start + width as usize)];
                                let dest_slice = &mut self.atlas_data[dest_start..(dest_start + width as usize)];
                                dest_slice.copy_from_slice(src_slice);
                            }
                        }

                        self.atlas_dirty = true;

                        let u1 = x as f32 / self.atlas_width as f32;
                        let v1 = y as f32 / self.atlas_height as f32;
                        let u2 = (x + width) as f32 / self.atlas_width as f32;
                        let v2 = (y + height) as f32 / self.atlas_height as f32;

                        self.glyph_cache.insert(cache_key, GlyphInfo {
                            uv: [u1, v1, u2, v2],
                            width,
                            height,
                            offset_x: image.placement.left,
                            offset_y: image.placement.top,
                        });

                        self.current_atlas_x += width + 2;
                        self.row_height = self.row_height.max(height);
                    }
                }

                if let Some(info) = self.glyph_cache.get(&cache_key) {
                    let x = pos.x + physical_glyph.x as f32 + info.offset_x as f32;
                    let y = pos.y + run.line_y as f32 + physical_glyph.y as f32 - info.offset_y as f32;

                    add_quad(&mut self.vertices, 
                        [x, y, info.width as f32, info.height as f32], 
                        text_color, 
                        info.uv
                    );
                }
            }
        }
        
        // Final sync if dirty
        self.ensure_atlas_texture();
    }
}

pub struct IcedRuntime<S, M> {
    state: RefCell<S>,
    update_fn: fn(&mut S, M) -> Command<M>,
    view_fn: fn(&S) -> iced_core::Element<'_, M, Theme, GpuRenderer>,
    renderer: RefCell<GpuRenderer>,
    
    interface_cache: RefCell<user_interface::Cache>,
    canvas_size: RefCell<(u32, u32)>,
    scale_factor: RefCell<f32>,
    cursor_position: RefCell<Point>,
    pending_events: RefCell<Vec<Event>>,
    needs_redrawing: RefCell<bool>,
    
    tasks: RefCell<Vec<BoxedFuture<M>>>,
    ui_pipeline: RefCell<Option<u64>>,
    background_texture: RefCell<Option<crate::rpc::services::ResourceHandle>>,
    vertex_buffer: RefCell<Option<crate::rpc::services::ResourceHandle>>,
    uniform_buffer: RefCell<Option<crate::rpc::services::ResourceHandle>>,
}

unsafe impl<S, M> Send for IcedRuntime<S, M> {}
unsafe impl<S, M> Sync for IcedRuntime<S, M> {}

impl<S: 'static, M: Send + 'static> IcedRuntime<S, M> {
    pub fn new(
        state: S, 
        update_fn: fn(&mut S, M) -> Command<M>,
        view_fn: fn(&S) -> iced_core::Element<'_, M, Theme, GpuRenderer>,
        _default_font: Font, 
        _font_data: Vec<(&'static str, &'static [u8])>
    ) -> Self {
        Self {
            state: RefCell::new(state),
            update_fn,
            view_fn,
            renderer: RefCell::new(GpuRenderer::new()),
            interface_cache: RefCell::new(user_interface::Cache::default()),
            canvas_size: RefCell::new((1024, 768)),
            scale_factor: RefCell::new(1.0),
            cursor_position: RefCell::new(Point::ORIGIN),
            pending_events: RefCell::new(Vec::new()),
            needs_redrawing: RefCell::new(true),
            tasks: RefCell::new(Vec::new()),
            ui_pipeline: RefCell::new(None),
            background_texture: RefCell::new(None),
            vertex_buffer: RefCell::new(None),
            uniform_buffer: RefCell::new(None),
        }
    }
}

impl<S: 'static, M: Send + 'static> RawIcedRuntime for IcedRuntime<S, M> {
    fn set_background_image(&self, handle: Option<crate::rpc::services::ResourceHandle>) {
        *self.background_texture.borrow_mut() = handle;
        *self.needs_redrawing.borrow_mut() = true;
    }
    fn tick(&self) -> anyhow::Result<()> {
        let mut new_messages = Vec::new();
        {
            let mut tasks = self.tasks.borrow_mut();
            if tasks.is_empty() { return Ok(()); }
            let waker = noop_waker_ref();
            let mut cx = Context::from_waker(waker);
            tasks.retain_mut(|task| {
                match task.as_mut().poll(&mut cx) {
                    Poll::Ready(maybe_msg) => {
                        if let Some(msg) = maybe_msg { new_messages.push(msg); }
                        false
                    },
                    Poll::Pending => true,
                }
            });
        }
        if !new_messages.is_empty() {
            let mut state = self.state.borrow_mut();
            let mut tasks = self.tasks.borrow_mut();
            for msg in new_messages {
                let command = (self.update_fn)(&mut state, msg);
                tasks.extend(command.0);
            }
            *self.needs_redrawing.borrow_mut() = true;
        }
        Ok(())
    }

    fn handle_event(&self, event_proto: UiEvent) -> anyhow::Result<()> {
        if let Some(ev) = event_proto.event {
            match ev {
                crate::rpc::ui::ui_event::Event::Resize(r) => { 
                    *self.canvas_size.borrow_mut() = (r.width, r.height);
                    *self.scale_factor.borrow_mut() = r.scale_factor;
                    *self.needs_redrawing.borrow_mut() = true;
                }
                crate::rpc::ui::ui_event::Event::CursorMoved(c) => {
                    let sf = *self.scale_factor.borrow();
                    let pos = Point::new(c.x / sf, c.y / sf);
                    *self.cursor_position.borrow_mut() = pos;
                    let mut events = self.pending_events.borrow_mut();
                    events.push(Event::Mouse(iced_core::mouse::Event::CursorMoved { position: pos }));
                }
                crate::rpc::ui::ui_event::Event::Click(c) => {
                    let sf = *self.scale_factor.borrow();
                    let _pos = Point::new(c.x / sf, c.y / sf);
                    let button = match c.button {
                        1 => iced_core::mouse::Button::Left,
                        2 => iced_core::mouse::Button::Right,
                        3 => iced_core::mouse::Button::Middle,
                        _ => iced_core::mouse::Button::Left,
                    };
                    let mut events = self.pending_events.borrow_mut();
                    if c.pressed {
                        events.push(Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)));
                    } else {
                        events.push(Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn render(&self) -> anyhow::Result<()> {
        let (width, height) = *self.canvas_size.borrow();
        if width == 0 || height == 0 { return Ok(()); }
        let sf = *self.scale_factor.borrow();
        let cursor_pos = *self.cursor_position.borrow();
        let cursor = iced_core::mouse::Cursor::Available(cursor_pos);
        let events = std::mem::take(&mut *self.pending_events.borrow_mut());

        let mut captured_messages = Vec::new();
        let viewport = Viewport::with_physical_size(Size::new(width, height), sf);
        let mut should_draw = *self.needs_redrawing.borrow() || !events.is_empty();

        let ui_cache = {
            let cache = std::mem::take(&mut *self.interface_cache.borrow_mut());
            let mut renderer_mut = self.renderer.borrow_mut();
            renderer_mut.clear();
            
            let state = self.state.borrow();
            
            let mut ui = UserInterface::build(
                (self.view_fn)(&state),
                viewport.logical_size(),
                cache,
                &mut *renderer_mut,
            );

            let mut clipboard = iced_core::clipboard::Null;
            let (ui_state, _) = ui.update(&events, cursor, &mut *renderer_mut, &mut clipboard, &mut captured_messages);
            
            if matches!(ui_state, user_interface::State::Outdated) || !captured_messages.is_empty() {
                should_draw = true;
            }

            if should_draw {
                let mut recorder = WgpuRecorder::new(width, height);
                
                // 1. Setup Pipeline
                if self.ui_pipeline.borrow().is_none() {
                    let shader_source = include_str!("shaders.wgsl");
                    if let Ok(sh) = crate::core::create_shader(shader_source, "GPU UI Shader") {
                        if let Ok(pip) = crate::core::create_pipeline(sh.id, "GPU UI Pipeline") {
                            *self.ui_pipeline.borrow_mut() = Some(pip.id);
                        }
                    }
                }

                // 2. Setup Uniforms (Resolution)
                if self.uniform_buffer.borrow().is_none() {
                    use crate::rpc::services::{GpuResourceRequest, GpuResourceResponse};
                    use crate::rpc::wgpu::CreateBuffer;
                    let req = GpuResourceRequest {
                        command: Some(crate::rpc::services::gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                            size: 16, usage: 64, mapped_at_creation: false
                        }))
                    };
                    if let Ok(res_bytes) = crate::rpc::host::call_service("system", "create_resource", req.encode_to_vec()) {
                        if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                            *self.uniform_buffer.borrow_mut() = res.handle;
                        }
                    }
                }

                if let Some(u_h) = &*self.uniform_buffer.borrow() {
                    // Передаем логические пиксели в шейдер, так как вершины Iced в логических пикселях
                    let logical_w = width as f32 / sf;
                    let logical_h = height as f32 / sf;
                    let res_data: [f32; 2] = [logical_w, logical_h];
                    let data = unsafe { std::slice::from_raw_parts(res_data.as_ptr() as *const u8, 8) };
                    let _ = crate::rpc::host::gpu_write_resource(u_h.id, 0, data);
                }

                // 3. Tessellate actual UI
                ui.draw(&mut *renderer_mut, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
                
                let mut final_vertices = Vec::new();
                
                // Background First (Zero-Copy)
                if let Some(_) = &*self.background_texture.borrow() {
                    add_quad(&mut final_vertices, [0.0, 0.0, viewport.logical_size().width, viewport.logical_size().height], [1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
                }
                
                // Then actual UI vertices captured by GpuRenderer
                final_vertices.extend(renderer_mut.vertices.iter().cloned());

                if !final_vertices.is_empty() { 
                    // 4. Update Vertex Buffer
                    if self.vertex_buffer.borrow().is_none() {
                        use crate::rpc::services::{GpuResourceRequest, GpuResourceResponse};
                        use crate::rpc::wgpu::CreateBuffer;
                        let req = GpuResourceRequest {
                            command: Some(crate::rpc::services::gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                                size: 1024 * 1024 * 4, usage: 32, mapped_at_creation: false
                            }))
                        };
                        if let Ok(res_bytes) = crate::rpc::host::call_service("system", "create_resource", req.encode_to_vec()) {
                            if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                                *self.vertex_buffer.borrow_mut() = res.handle;
                            }
                        }
                    }

                    if let (Some(pipeline), Some(v_h), Some(u_h)) = (*self.ui_pipeline.borrow(), &*self.vertex_buffer.borrow(), &*self.uniform_buffer.borrow()) {
                        let data = unsafe { std::slice::from_raw_parts(final_vertices.as_ptr() as *const u8, final_vertices.len() * 32) };
                        let _ = crate::rpc::host::gpu_write_resource(v_h.id, 0, data);

                        recorder.set_pipeline(pipeline);
                        recorder.set_vertex_buffer(0, v_h.id, 0, (final_vertices.len() * 32) as u64);
                        recorder.set_bind_group(1, u_h.id);

                        // Use Atlas Texture
                        renderer_mut.ensure_atlas_texture();
                        if let Some(atlas_id) = renderer_mut.atlas_texture_id {
                             recorder.set_bind_group(0, atlas_id);
                        } else if let Some(bg) = &*self.background_texture.borrow() {
                            recorder.set_bind_group(0, bg.id);
                        }
                        
                        recorder.draw(0..final_vertices.len() as u32, 0..1);
                    }
                    let _ = recorder.submit();
                }
            }
            ui.into_cache()
        };

        if !captured_messages.is_empty() {
            let mut state_mut = self.state.borrow_mut();
            let mut tasks = self.tasks.borrow_mut();
            for message in captured_messages {
                let command = (self.update_fn)(&mut state_mut, message);
                tasks.extend(command.0);
            }
            *self.needs_redrawing.borrow_mut() = true;
        }
        *self.interface_cache.borrow_mut() = ui_cache;
        *self.needs_redrawing.borrow_mut() = false;
        Ok(())
    }
}
