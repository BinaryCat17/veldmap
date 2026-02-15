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
        *plugin.is_layout_dirty.borrow_mut() = true;
        *plugin.needs_redrawing.borrow_mut() = true;
    }
    Ok(SetViewResponse {})
}

pub fn handle_ui_event(state: &mut LocalState, req: HandleUiEventRequest) -> anyhow::Result<HandleUiEventResponse> {
    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    let mut messages = Vec::new();
    if let Some(event_proto) = req.event {
        // active_surface_id is now constant for single-window mode
        
        if let Some(ev) = event_proto.event {
            match ev {
                app_proto::ui_event::Event::Resize(r) => {
                    *plugin.canvas_size.borrow_mut() = (r.width, r.height);
                    *plugin.scale_factor.borrow_mut() = r.scale_factor;
                    *plugin.needs_redrawing.borrow_mut() = true;
                }
                app_proto::ui_event::Event::Scroll(s) => {
                    let mut vel = plugin.scroll_velocity.borrow_mut();
                    if (s.delta_y > 0.0 && vel.y < 0.0) || (s.delta_y < 0.0 && vel.y > 0.0) {
                        vel.y = 0.0;
                    }
                    let dy = s.delta_y.signum() * (s.delta_y.abs().powf(0.4) * 24.0);
                    let dx = s.delta_x.signum() * (s.delta_x.abs().powf(0.4) * 24.0);
                    vel.x += dx;
                    vel.y += dy;
                    vel.x = vel.x.clamp(-3000.0, 3000.0);
                    vel.y = vel.y.clamp(-3000.0, 3000.0);
                    *plugin.needs_redrawing.borrow_mut() = true;
                }
                app_proto::ui_event::Event::Frame(f) => {
                    let mut vel = plugin.scroll_velocity.borrow_mut();
                    if vel.x.abs() > 0.5 || vel.y.abs() > 0.5 {
                        let friction = 0.85f32; 
                        let factor = 1.0 - friction.powf(f.dt * 60.0);
                        let scroll_amount_x = vel.x * factor;
                        let scroll_amount_y = vel.y * factor;
                        plugin.pending_events.borrow_mut().push(Event::Mouse(iced_core::mouse::Event::WheelScrolled { 
                            delta: iced_core::mouse::ScrollDelta::Pixels { x: scroll_amount_x, y: scroll_amount_y } 
                        }));
                        vel.x -= scroll_amount_x;
                        vel.y -= scroll_amount_y;
                        *plugin.needs_redrawing.borrow_mut() = true;
                    } else {
                        vel.x = 0.0;
                        vel.y = 0.0;
                    }

                    // ОПТИМИЗАЦИЯ: вызываем рендер только если есть события или флаг перерисовки
                    if !plugin.pending_events.borrow().is_empty() || *plugin.needs_redrawing.borrow() || *plugin.is_layout_dirty.borrow() {
                        messages = render_plugin(plugin, &mut state.renderer, &req.plugin_id, state.surface_format)?;
                    }
                }
                _ => {
                    let iced_ev = convert_event(ev, *plugin.scale_factor.borrow());
                    if let Event::Mouse(iced_core::mouse::Event::CursorMoved { position }) = iced_ev {
                        *plugin.cursor_position.borrow_mut() = position;
                    }
                    plugin.pending_events.borrow_mut().push(iced_ev);
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
            let button = match c.button { 1 => iced_core::mouse::Button::Left, 2 => iced_core::mouse::Button::Right, 3 => iced_core::mouse::Button::Middle, _ => iced_core::mouse::Button::Left };
            if c.pressed { Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)) }
            else { Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)) }
        }
        _ => Event::Window(iced_core::window::Event::RedrawRequested(std::time::Instant::now())),
    }
}

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, surface_format: i32) -> anyhow::Result<Vec<UiEventResponse>> {
    let (width, height) = *plugin.canvas_size.borrow();
    if width == 0 || height == 0 { return Ok(Vec::new()); }
    
    let events = std::mem::take(&mut *plugin.pending_events.borrow_mut());
    
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
    let _ = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);
    
    ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);

    // COMMAND DIFFING: Проверяем, изменилось ли что-то в командах отрисовки или атласе
    let mut last_cmds = plugin.last_draw_commands.borrow_mut();
    let mut last_verts = plugin.last_vertices.borrow_mut();
    let mut is_layout_dirty = plugin.is_layout_dirty.borrow_mut();

    let commands_changed = *last_cmds != renderer.draw_commands || 
                           *last_verts != renderer.vertices ||
                           renderer.is_atlas_dirty();

    if commands_changed || *is_layout_dirty {
        execute_gpu_commands(plugin, renderer, width, height, sf, plugin_id, surface_format)?;
        
        // Обновляем кэш только если была реальная отправка
        *last_cmds = renderer.draw_commands.clone();
        *last_verts = renderer.vertices.clone();
        *is_layout_dirty = false;
    }

    *plugin.needs_redrawing.borrow_mut() = false;
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

fn execute_gpu_commands(plugin: &PluginUiState, renderer: &mut GpuRenderer, width: u32, height: u32, sf: f32, _plugin_id: &str, surface_format: i32) -> anyhow::Result<()> {
    ensure_gpu_resources(plugin, renderer, surface_format)?;

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
            let (offset, data) = renderer.atlas_dirty_range();
            let _ = gpu_write_resource(tid, offset, data);
            renderer.mark_atlas_clean();
        }
    }

    if !renderer.vertices.is_empty() {
        let mut vertex_buffer = plugin.vertex_buffer.borrow_mut();
        if vertex_buffer.is_none() {
            let req = GpuResourceRequest {
                instance_id: 0,
                command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                    size: 1024 * 1024 * 8, usage: 32, mapped_at_creation: false, readonly: false
                }))
            };
            let res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?;
            *vertex_buffer = res.handle.map(OwnedResource::new);
        }

        let mut index_buffer = plugin.index_buffer.borrow_mut();
        if index_buffer.is_none() {
            let req = GpuResourceRequest {
                instance_id: 0,
                command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                    size: 1024 * 1024 * 2, usage: 16, mapped_at_creation: false, readonly: false
                }))
            };
            let res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", req.encode_to_vec())?[..])?;
            *index_buffer = res.handle.map(OwnedResource::new);
        }

        if let (Some(pipeline), Some(ref v_h), Some(ref i_h), Some(ref u_h)) = (*plugin.ui_pipeline.borrow(), &*vertex_buffer, &*index_buffer, &*plugin.uniform_buffer.borrow()) {
            let vertex_size = std::mem::size_of::<crate::renderer::Vertex>();
            let v_data = unsafe { std::slice::from_raw_parts(renderer.vertices.as_ptr() as *const u8, renderer.vertices.len() * vertex_size) };
            let _ = gpu_write_resource(v_h.id(), 0, v_data);

            let i_data = unsafe { std::slice::from_raw_parts(renderer.indices.as_ptr() as *const u8, renderer.indices.len() * 2) };
            let _ = gpu_write_resource(i_h.id(), 0, i_data);

            recorder.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

            let mut current_index_offset = 0;
            for cmd in &renderer.draw_commands {
                match cmd {
                    DrawCmd::Quads { count } => {
                        recorder.set_pipeline(pipeline);
                        recorder.set_vertex_buffer(0, v_h.id(), 0, (renderer.vertices.len() * vertex_size) as u64);
                        recorder.set_index_buffer(i_h.id(), IndexFormat::IdxUint16 as u32, 0, (renderer.indices.len() * 2) as u64);
                        recorder.set_bind_group(1, u_h.id());
                        if let Some(atlas_bg) = renderer.atlas_bind_group_id {
                            recorder.set_bind_group(0, atlas_bg);
                        }
                        recorder.draw_indexed(current_index_offset..(*count + current_index_offset), 0, 0..1);
                        current_index_offset += *count;
                    }
                    DrawCmd::Scissor { x, y, width, height } => {
                        recorder.set_scissor_rect(*x, *y, *width, *height);
                    }
                    DrawCmd::ExternalImage { .. } => {}
                }
            }
        }
    }

    // Direct to surface
    // We don't clear here, host will clear the surface if needed
    let surface_id = plugin.active_surface_id;
    let _ = recorder.submit(surface_id, None);
    let _ = veldsdk::app::AppBridge::display_frame(veldsdk::rpc::core::ResourceHandle { id: surface_id, ..Default::default() }, width, height);

    Ok(())
}

fn ensure_gpu_resources(plugin: &PluginUiState, renderer: &mut GpuRenderer, surface_format: i32) -> anyhow::Result<()> {
    // 1. Atlas Layout (Group 0)
    if renderer.bgl_id.is_none() {
        let req = GpuResourceRequest {
            instance_id: 0,
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

    // 2. Uniform Layout (Group 1)
    let mut uniform_layout_id = plugin.uniform_layout_id.borrow_mut();
    if uniform_layout_id.is_none() {
        let bgl_req = GpuResourceRequest {
            instance_id: 0,
            command: Some(gpu_resource_request::Command::CreateBindGroupLayout(CreateBindGroupLayout {
                label: "UI Uniform BGL".into(),
                entries: vec![BindGroupLayoutEntry {
                    binding: 0, visibility: 3, ty: Some(bind_group_layout_entry::Ty::Buffer(BufferBindingLayout { r#type: 1, ..Default::default() }))
                }]
            }))
        };
        let bgl_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", bgl_req.encode_to_vec())?[..])?;
        *uniform_layout_id = bgl_res.handle.map(|h| h.id);
    }

    // 3. Pipeline (depends on layouts)
    let mut ui_pipeline = plugin.ui_pipeline.borrow_mut();
    if ui_pipeline.is_none() {
        let shader_source = include_str!("shaders.wgsl");
        let sh_req = GpuResourceRequest {
            instance_id: 0,
            command: Some(gpu_resource_request::Command::CreateShader(CreateShaderModule {
                source: shader_source.into(), label: "UI Shader".into()
            }))
        };
        let sh_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", sh_req.encode_to_vec())?[..])?;
        if let Some(sh) = sh_res.handle {
            let mut bgl_ids = Vec::new();
            if let Some(id) = renderer.bgl_id { bgl_ids.push(id); }
            if let Some(id) = *uniform_layout_id { bgl_ids.push(id); }

            let pip_req = GpuResourceRequest {
                instance_id: 0,
                command: Some(gpu_resource_request::Command::CreatePipeline(CreateRenderPipeline {
                    shader_id: sh.id, label: "UI Pipeline".into(), 
                    vertex_entry: "vs_main".into(), fragment_entry: "fs_main".into(),
                    target_format: surface_format,
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
                    bind_group_layout_ids: bgl_ids,
                    primitive_topology: PrimitiveTopology::TopologyTriangleList as i32,
                    front_face: FrontFace::Ccw as i32,
                    cull_mode: CullMode::None as i32,
                    ..Default::default()
                }))
            };
            let pip_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", pip_req.encode_to_vec())?[..])?;
            *ui_pipeline = pip_res.handle.map(|h| h.id);
        }
    }

    // 4. Buffers and Groups
    let mut uniform_buffer = plugin.uniform_buffer.borrow_mut();
    let mut uniform_buffer_id = plugin.uniform_buffer_id.borrow_mut();
    if uniform_buffer.is_none() && uniform_layout_id.is_some() {
        let buf_req = GpuResourceRequest {
            instance_id: 0,
            command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
                size: 16, usage: 64, mapped_at_creation: false, readonly: false
            }))
        };
        let buf_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", buf_req.encode_to_vec())?[..])?;
        if let Some(bh) = buf_res.handle {
            *uniform_buffer_id = Some(bh.id);
            let bg_req = GpuResourceRequest {
                instance_id: 0,
                command: Some(gpu_resource_request::Command::CreateBindGroup(CreateBindGroup {
                    layout_id: uniform_layout_id.unwrap(), entries: vec![BindGroupEntry { binding: 0, resource: Some(bind_group_entry::Resource::BufferId(bh.id)) }], label: "UI Uniform BG".into()
                }))
            };
            let bg_res = GpuResourceResponse::decode(&call_service("wgpu", "create_resource", bg_req.encode_to_vec())?[..])?;
            *uniform_buffer = bg_res.handle.map(OwnedResource::new);
        }
    }

    if renderer.atlas_texture_id.is_none() {
        let (w, h) = renderer.atlas_dimensions();
        let req = GpuResourceRequest {
            instance_id: 0,
            command: Some(gpu_resource_request::Command::CreateTexture(CreateTexture {
                width: w, height: h, format: TextureFormat::TexRgba8Unorm as i32, usage: 2 | 4, dimension: 1, mip_level_count: 1, sample_count: 1, depth_or_array_layers: 1, readonly: false
            }))
        };
        if let Ok(res_bytes) = call_service("wgpu", "create_resource", req.encode_to_vec()) {
            if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                renderer.atlas_texture_id = res.handle.map(|h| h.id);
                renderer.mark_atlas_dirty(); // Force upload of atlas data
            }
        }
    }
    
    if renderer.atlas_bind_group_id.is_none() && renderer.atlas_texture_id.is_some() && renderer.bgl_id.is_some() {
        let sampler_req = GpuResourceRequest {
            instance_id: 0,
            command: Some(gpu_resource_request::Command::CreateSampler(CreateSampler { mag_filter: FilterMode::FiltLinear as i32, min_filter: FilterMode::FiltLinear as i32, ..Default::default() }))
        };
        let sampler_id = call_service("wgpu", "create_resource", sampler_req.encode_to_vec()).ok().and_then(|b| GpuResourceResponse::decode(&b[..]).ok()).and_then(|r| r.handle).map(|h| h.id).unwrap_or(0);
        let req = GpuResourceRequest {
            instance_id: 0,
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
