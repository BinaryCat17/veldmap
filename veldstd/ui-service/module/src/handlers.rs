use veld_ui::proto::*;
use crate::state::{PluginUiState, LocalState};
use veldsdk::rpc::app as app_proto;
use crate::renderer::{GpuRenderer, DrawCmd};
use crate::converter;
use iced_core::{Point, Event, Size, Theme};
use iced_runtime::UserInterface;
use iced_graphics::Viewport;
use veldsdk::rpc::wgpu::*;
use veldsdk::rpc::host::{call_service, gpu_write_resource};
use prost::Message;
use veldsdk::wgpu::wgpu_proxy::WgpuRecorder;
use veldsdk::OwnedResource;

pub fn handle_set_view(state: &mut LocalState, req: SetViewRequest) -> anyhow::Result<SetViewResponse> {
    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    if let Some(l) = req.layout {
        plugin.layout = l;
        plugin.is_layout_dirty = true;
        *plugin.needs_redrawing.borrow_mut() = true;
    }
    Ok(SetViewResponse {})
}

pub fn handle_ui_event(state: &mut LocalState, req: HandleUiEventRequest) -> anyhow::Result<HandleUiEventResponse> {
    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    let mut messages = Vec::new();
    if let Some(event_proto) = req.event {
        if let Some(ev) = event_proto.event {
            match ev {
                app_proto::ui_event::Event::Resize(r) => {
                    *plugin.canvas_size.borrow_mut() = (r.width, r.height);
                    *plugin.scale_factor.borrow_mut() = r.scale_factor;
                    *plugin.ui_texture.borrow_mut() = None;
                    *plugin.needs_redrawing.borrow_mut() = true;
                }
                app_proto::ui_event::Event::Frame(_f) => {
                    messages = render_plugin(plugin, &mut state.renderer, &req.plugin_id)?;
                }
                _ => {
                    plugin.pending_events.borrow_mut().push(convert_event(ev, *plugin.scale_factor.borrow()));
                    *plugin.needs_redrawing.borrow_mut() = true;
                }
            }
        }
    }
    Ok(HandleUiEventResponse { messages })
}

fn convert_event(ev: app_proto::ui_event::Event, sf: f32) -> Event {
    match ev {
        app_proto::ui_event::Event::CursorMoved(c) => Event::Mouse(iced_core::mouse::Event::CursorMoved { position: Point::new(c.x / sf, c.y / sf) }),
        app_proto::ui_event::Event::Click(c) => {
            let pos = Point::new(c.x / sf, c.y / sf);
            let button = match c.button { 1 => iced_core::mouse::Button::Left, 2 => iced_core::mouse::Button::Right, 3 => iced_core::mouse::Button::Middle, _ => iced_core::mouse::Button::Left };
            if c.pressed { Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)) }
            else { Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)) }
        }
        app_proto::ui_event::Event::Scroll(s) => Event::Mouse(iced_core::mouse::Event::WheelScrolled { delta: iced_core::mouse::ScrollDelta::Pixels { x: s.delta_x, y: s.delta_y } }),
        _ => Event::Window(iced_core::window::Event::RedrawRequested(std::time::Instant::now())),
    }
}

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str) -> anyhow::Result<Vec<UiEventResponse>> {
    let (width, height) = *plugin.canvas_size.borrow();
    if width == 0 || height == 0 { return Ok(Vec::new()); }
    
    let mut needs_redrawing = plugin.needs_redrawing.borrow_mut();
    let events = std::mem::take(&mut *plugin.pending_events.borrow_mut());
    
    if !*needs_redrawing && events.is_empty() {
        if let Some(tex) = &*plugin.ui_texture.borrow() {
            let _ = veldsdk::app::AppBridge::display_frame(tex.handle(), width, height);
            return Ok(Vec::new());
        }
    }

    let sf = *plugin.scale_factor.borrow();
    renderer.update_params(width, height, sf);
    let cursor_pos = *plugin.cursor_position.borrow();
    let cursor = iced_core::mouse::Cursor::Available(cursor_pos);
    let viewport = Viewport::with_physical_size(Size::new(width, height), sf.into());
    let mut captured_messages = Vec::new();

    renderer.clear();
    let element = converter::convert_layout(&plugin.layout);
    
    let cache = plugin.interface_cache.replace(iced_runtime::user_interface::Cache::default());
    let _guard = crate::renderer::ScopeGuard::new(&mut renderer.font_system, &mut renderer.swash_cache);

    let mut ui = UserInterface::build(
        element,
        viewport.logical_size(),
        cache,
        renderer,
    );
    
    let mut clipboard = iced_core::clipboard::Null;
    let (ui_state, _) = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);
    
    if *needs_redrawing || !events.is_empty() || matches!(ui_state, iced_runtime::user_interface::State::Outdated) {
        ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
        execute_gpu_commands(plugin, renderer, width, height, sf, plugin_id)?;
        *needs_redrawing = false;
    } else {
        if let Some(tex) = &*plugin.ui_texture.borrow() {
            let _ = veldsdk::app::AppBridge::display_frame(tex.handle(), width, height);
        }
    }
    
    plugin.interface_cache.replace(ui.into_cache());
    
    let mut responses = Vec::new();
    for msg in captured_messages {
        responses.push(UiEventResponse {
            plugin_id: plugin_id.to_string(),
            message_tag: msg.tag,
            value: msg.value,
        });
    }
    
    Ok(responses)
}

fn execute_gpu_commands(plugin: &PluginUiState, renderer: &mut GpuRenderer, width: u32, height: u32, sf: f32, _plugin_id: &str) -> anyhow::Result<()> {
    ensure_gpu_resources(plugin, renderer)?;

    let mut recorder = WgpuRecorder::new(width, height);
    let logical_w = width as f32 / sf;
    let logical_h = height as f32 / sf;

    if let Some(u_id) = *plugin.uniform_buffer_id.borrow() {
        let res_data: [f32; 2] = [logical_w, logical_h];
        let data = unsafe { std::slice::from_raw_parts(res_data.as_ptr() as *const u8, 8) };
        let _ = gpu_write_resource(u_id, 0, data);
    }

    if renderer.is_atlas_dirty() {
        if let Some(tid) = renderer.atlas_texture_id {
            let (_, _, data) = renderer.atlas_data();
            let _ = gpu_write_resource(tid, 0, data);
            renderer.mark_atlas_clean();
        }
    }

    if !renderer.vertices.is_empty() {
        let mut vertex_buffer = plugin.vertex_buffer.borrow_mut();
        if vertex_buffer.is_none() {
            let req = GpuResourceRequest {
                command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                    size: 1024 * 1024 * 8, usage: 32, mapped_at_creation: false, readonly: false
                }))
            };
            let res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?;
            *vertex_buffer = res.handle.map(OwnedResource::new);
        }

        if let (Some(pipeline), Some(ref v_h), Some(ref u_h)) = (*plugin.ui_pipeline.borrow(), &*vertex_buffer, &*plugin.uniform_buffer.borrow()) {
            let vertex_size = std::mem::size_of::<crate::renderer::Vertex>();
            let data = unsafe { std::slice::from_raw_parts(renderer.vertices.as_ptr() as *const u8, renderer.vertices.len() * vertex_size) };
            let _ = gpu_write_resource(v_h.id(), 0, data);

            recorder.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

            let mut current_vertex_offset = 0;
            for cmd in &renderer.draw_commands {
                match cmd {
                    DrawCmd::Quads { count } => {
                        recorder.set_pipeline(pipeline);
                        recorder.set_vertex_buffer(0, v_h.id(), (current_vertex_offset as usize * vertex_size) as u64, (*count as usize * vertex_size) as u64);
                        recorder.set_bind_group(1, u_h.id());
                        if let Some(atlas_bg) = renderer.atlas_bind_group_id {
                            recorder.set_bind_group(0, atlas_bg);
                        }
                        recorder.draw(0..*count, 0..1);
                        current_vertex_offset += *count;
                    }
                    DrawCmd::Scissor { x, y, width, height } => {
                        recorder.set_scissor_rect(*x, *y, *width, *height);
                    }
                    DrawCmd::ExternalImage { .. } => {}
                }
            }
        }
    }

    let mut ui_texture = plugin.ui_texture.borrow_mut();
    if ui_texture.is_none() {
        let req = GpuResourceRequest {
            command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
                width, height, format: TextureFormat::TexRgba8Unorm as i32, usage: 16 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1, readonly: false
            }))
        };
        let res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?;
        *ui_texture = res.handle.map(OwnedResource::new);
    }

    if let Some(ui_tex) = &*ui_texture {
        let _ = recorder.submit(ui_tex.id(), Some(veldsdk::rpc::wgpu::GpuColor { r: 0.05, g: 0.05, b: 0.07, a: 1.0 }));
        let _ = veldsdk::app::AppBridge::display_frame(ui_tex.handle(), width, height);
    }

    Ok(())
}

fn ensure_gpu_resources(plugin: &PluginUiState, renderer: &mut GpuRenderer) -> anyhow::Result<()> {
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
                    target_format: TextureFormat::TexRgba8Unorm as i32,
                    vertex_layouts: vec![VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::renderer::Vertex>() as u64,
                        step_mode: StepMode::StepVertex as i32,
                        attributes: vec![
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 0, shader_location: 0 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x4 as i32, offset: 8, shader_location: 1 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 24, shader_location: 2 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 32, shader_location: 3 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x2 as i32, offset: 40, shader_location: 4 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 48, shader_location: 5 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 52, shader_location: 6 },
                            VertexAttribute { format: VertexFormat::VtxFloat32 as i32, offset: 56, shader_location: 7 },
                            VertexAttribute { format: VertexFormat::VtxFloat32x4 as i32, offset: 60, shader_location: 8 },
                        ],
                    }],
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
                size: 16, usage: 64, mapped_at_creation: false, readonly: false
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
                let bg_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", bg_req.encode_to_vec())?[..])?;
                *uniform_buffer = bg_res.handle.map(OwnedResource::new);
            }
        }
    }

    if renderer.bgl_id.is_none() {
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
                renderer.bgl_id = res.handle.map(|h| h.id);
            }
        }
    }

    if renderer.atlas_texture_id.is_none() {
        let (w, h, _) = renderer.atlas_data();
        let req = GpuResourceRequest {
            command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
                width: w, height: h, format: TextureFormat::TexRgba8Unorm as i32, usage: 2 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1, readonly: false
            }))
        };
        if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                renderer.atlas_texture_id = res.handle.map(|h| h.id);
            }
        }
    }
    
    if renderer.atlas_bind_group_id.is_none() && renderer.atlas_texture_id.is_some() && renderer.bgl_id.is_some() {
        let sampler_req = GpuResourceRequest {
            command: Some(gpu_resource_request::Command::CreateSampler(CreateSampler { mag_filter: FilterMode::FiltLinear as i32, min_filter: FilterMode::FiltLinear as i32, ..Default::default() }))
        };
        let sampler_id = call_service("wgpu", "create_resource", sampler_req.encode_to_vec()).ok().and_then(|b| GpuResourceResponse::decode(&b[..]).ok()).and_then(|r| r.handle).map(|h| h.id).unwrap_or(0);
        let req = GpuResourceRequest {
            command: Some(gpu_resource_request::Command::CreateBindGroup(CreateBindGroup {
                layout_id: renderer.bgl_id.unwrap(), entries: vec![
                    BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::TextureViewId(renderer.atlas_texture_id.unwrap())) },
                    BindGroupEntry { binding: 1, resource: Some(bind_group_entry::Resource::SamplerId(sampler_id)) },
                ],
                label: "Iced Atlas BG".into(),
            }))
        };
        if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                renderer.atlas_bind_group_id = res.handle.map(|h| h.id);
            }
        }
    }

    Ok(())
}
