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
    
    match req.update {
        Some(set_view_request::Update::FullLayout(l)) => {
            plugin.layout = l;
            *plugin.is_layout_dirty.borrow_mut() = true;
            *plugin.needs_redrawing.borrow_mut() = true;
        }
        Some(set_view_request::Update::Patch(patch)) => {
            if let Some(ref mut root) = plugin.layout.root {
                for update in patch.updates {
                    if let Some(new_widget) = update.new_widget {
                        apply_widget_update(root, update.widget_id, new_widget);
                    }
                }
                plugin.layout.width = patch.width;
                plugin.layout.height = patch.height;
                plugin.layout.hash = patch.new_hash;
                *plugin.is_layout_dirty.borrow_mut() = true;
                *plugin.needs_redrawing.borrow_mut() = true;
            }
        }
        None => {}
    }
    Ok(SetViewResponse {})
}

fn apply_widget_update(current: &mut Widget, id: u64, new_w: Widget) -> bool {
    if current.id == id {
        *current = new_w;
        return true;
    }

    match &mut current.r#type {
        Some(widget::Type::Column(c)) => { for child in &mut c.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Row(r)) => { for child in &mut r.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Stack(s)) => { for child in &mut s.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Container(c)) => { if let Some(child) = &mut c.child { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Scrollable(s)) => { if let Some(child) = &mut s.content { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Tooltip(t)) => { if let Some(child) = &mut t.content { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Button(b)) => { if let Some(child) = &mut b.child { return apply_widget_update(child, id, new_w); } }
        _ => {}
    }
    false
}

pub fn handle_ui_event(state: &mut LocalState, req: HandleUiEventRequest) -> anyhow::Result<HandleUiEventResponse> {
    let mut messages = Vec::new();
    if let Some(event_proto) = req.event {
        messages = process_ui_event_recursive(state, &req.plugin_id, event_proto)?;
    }
    Ok(HandleUiEventResponse { messages })
}

fn process_ui_event_recursive(state: &mut LocalState, plugin_id: &str, mut req_event: app_proto::UiEvent) -> anyhow::Result<Vec<UiEventResponse>> {
    let mut messages = Vec::new();

    // 1. Сначала обрабатываем вложенные события (скролл, клики), чтобы 
    // Frame (если он есть) увидел уже обновленное состояние скорости.
    // Забираем sub_events, чтобы не держать заимствование req_event.
    let sub_events = std::mem::take(&mut req_event.sub_events);
    for sub_event in sub_events {
        let mut msgs = process_ui_event_recursive(state, plugin_id, sub_event)?;
        messages.append(&mut msgs);
    }

    let plugin = state.plugins.entry(plugin_id.to_string()).or_insert_with(PluginUiState::new);

    // 2. Обрабатываем основное событие
    if let Some(ev) = req_event.event {
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
                
                // Используем корень 6-й степени для экстремального сглаживания разницы между 9 и 120.
                // При 9: 9^(1/6) * 24 = ~35. При 120: 120^(1/6) * 24 = ~53.
                // Разница между микро-шагом и полным шагом теперь минимальна (~1.5 раза).
                let factor = 24.0;
                let dy = s.delta_y.signum() * s.delta_y.abs().powf(1.0 / 6.0) * factor;
                let dx = s.delta_x.signum() * s.delta_x.abs().powf(1.0 / 6.0) * factor;
                
                vel.x += dx;
                vel.y += dy;
                vel.x = vel.x.clamp(-3000.0, 3000.0);
                vel.y = vel.y.clamp(-3000.0, 3000.0);
                *plugin.needs_redrawing.borrow_mut() = true;
            }
            app_proto::ui_event::Event::Frame(f) => {
                {
                    let mut vel = plugin.scroll_velocity.borrow_mut();
                    if vel.x.abs() > 0.1 || vel.y.abs() > 0.1 {
                        let monitor_fps = *plugin.monitor_fps.borrow() as f32;
                        let friction = 0.92f32; // Более вязкая и плавная инерция
                        
                        // Используем monitor_fps для стабильной нормализации трения
                        let factor = 1.0 - friction.powf(f.dt * monitor_fps.max(60.0));
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
                }

                if !plugin.pending_events.borrow().is_empty() || *plugin.needs_redrawing.borrow() || *plugin.is_layout_dirty.borrow() {
                    let mut msgs = render_plugin(plugin, &mut state.renderer, plugin_id, state.surface_format)?;
                    messages.append(&mut msgs);
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

    Ok(messages)
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
    let render_start = std::time::Instant::now();
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
    let conv_start = std::time::Instant::now();
    let element = converter::convert_layout(&plugin.layout);
    let conv_time = conv_start.elapsed();
    
    let cache = plugin.interface_cache.replace(iced_runtime::user_interface::Cache::default());
    let _guard = crate::renderer::ScopeGuard::new(&mut renderer.font_system, &mut renderer.swash_cache);

    let ui_build_start = std::time::Instant::now();
    let mut ui = UserInterface::build(
        element,
        viewport.logical_size(),
        cache,
        renderer,
    );
    let ui_build_time = ui_build_start.elapsed();
    
    let mut clipboard = iced_core::clipboard::Null;
    let ui_update_start = std::time::Instant::now();
    let _ = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);
    let ui_update_time = ui_update_start.elapsed();
    
    let ui_draw_start = std::time::Instant::now();
    ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
    let ui_draw_time = ui_draw_start.elapsed();

    // COMMAND DIFFING: Проверяем, изменилось ли что-то в командах отрисовки или атласе
    let mut last_cmds = plugin.last_draw_commands.borrow_mut();
    let mut last_verts = plugin.last_vertices.borrow_mut();
    let mut is_layout_dirty = plugin.is_layout_dirty.borrow_mut();
    let needs_redrawing = *plugin.needs_redrawing.borrow();

    let commands_changed = *last_cmds != renderer.draw_commands || 
                           *last_verts != renderer.vertices ||
                           renderer.is_atlas_dirty();

    let gpu_exec_start = std::time::Instant::now();
    if commands_changed || *is_layout_dirty || needs_redrawing {
        let mut request_next = false;
        if plugin.scroll_velocity.borrow().x.abs() > 0.5 || plugin.scroll_velocity.borrow().y.abs() > 0.5 {
            request_next = true;
        }

        execute_gpu_commands(plugin, renderer, width, height, sf, plugin_id, surface_format, request_next)?;
        
        // Обновляем кэш только если была реальная отправка
        *last_cmds = renderer.draw_commands.clone();
        *last_verts = renderer.vertices.clone();
        *is_layout_dirty = false;
    } else {
        // Если мы решили не рисовать, но нам пришел Frame, мы всё равно должны 
        // сообщить хосту параметры монитора, если это был первый кадр или что-то изменилось.
        // Но обычно здесь просто ничего не происходит.
    }
    let gpu_exec_time = gpu_exec_start.elapsed();

    let total_time = render_start.elapsed();
    
    // Accumulate Stats
    {
        let mut count = plugin.perf_count.borrow_mut();
        *count += 1;
        *plugin.perf_total.borrow_mut() += total_time.as_micros();
        *plugin.perf_conv.borrow_mut() += conv_time.as_micros();
        *plugin.perf_build.borrow_mut() += ui_build_time.as_micros();
        *plugin.perf_update.borrow_mut() += ui_update_time.as_micros();
        *plugin.perf_draw.borrow_mut() += ui_draw_time.as_micros();
        *plugin.perf_gpu.borrow_mut() += gpu_exec_time.as_micros();

        let mut last_log = plugin.perf_last_log.borrow_mut();
        if last_log.map(|t| t.elapsed().as_secs() >= 5).unwrap_or(true) {
             if *count > 0 {
                 let avg_total = *plugin.perf_total.borrow() as f64 / 1000.0 / *count as f64;
                 let avg_conv = *plugin.perf_conv.borrow() as f64 / 1000.0 / *count as f64;
                 let avg_build = *plugin.perf_build.borrow() as f64 / 1000.0 / *count as f64;
                 let avg_update = *plugin.perf_update.borrow() as f64 / 1000.0 / *count as f64;
                 let avg_draw = *plugin.perf_draw.borrow() as f64 / 1000.0 / *count as f64;
                 let avg_gpu = *plugin.perf_gpu.borrow() as f64 / 1000.0 / *count as f64;
                 
                 log::info!(target: "veldmap_perf", "UI Render (5s avg) {}: count={}, total={:.2}ms, conv={:.2}ms, build={:.2}ms, update={:.2}ms, draw={:.2}ms, gpu={:.2}ms | Host: {}Hz, {:.1}FPS", 
                     plugin_id, *count, avg_total, avg_conv, avg_build, avg_update, avg_draw, avg_gpu,
                     *plugin.monitor_fps.borrow(), *plugin.actual_fps.borrow());
             }
             
             *count = 0;
             *plugin.perf_total.borrow_mut() = 0;
             *plugin.perf_conv.borrow_mut() = 0;
             *plugin.perf_build.borrow_mut() = 0;
             *plugin.perf_update.borrow_mut() = 0;
             *plugin.perf_draw.borrow_mut() = 0;
             *plugin.perf_gpu.borrow_mut() = 0;
             *last_log = Some(std::time::Instant::now());
        }
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

fn execute_gpu_commands(plugin: &PluginUiState, renderer: &mut GpuRenderer, width: u32, height: u32, sf: f32, _plugin_id: &str, surface_format: i32, request_next_frame: bool) -> anyhow::Result<()> {
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
    
    if let Ok(res) = veldsdk::app::AppBridge::display_frame(
        veldsdk::rpc::core::ResourceHandle { id: surface_id, ..Default::default() }, 
        width, height, 
        request_next_frame
    ) {
        *plugin.monitor_fps.borrow_mut() = res.monitor_fps;
        *plugin.actual_fps.borrow_mut() = res.actual_fps;
    }

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
