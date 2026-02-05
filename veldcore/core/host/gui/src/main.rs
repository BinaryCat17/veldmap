#![recursion_limit = "512"]
use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
};
use crate::app_service::{AppCommand, AppService};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    window::WindowBuilder,
};
use tokio::sync::mpsc;
use std::sync::Arc;
use prost::Message;

use std::sync::atomic::{AtomicBool, Ordering};

mod app_service;

fn execute_render_commands<'a>(
    rp: &mut wgpu::RenderPass<'a>,
    command_buffer: &'a veldmap_host_core::wgpu::CommandBuffer,
    resources: &'a veldmap_host_core::resources::ResourceManager,
    bind_group_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> anyhow::Result<()> {
    use veldmap_host_core::wgpu::wgpu_command::Command;

    for wgpu_cmd in &command_buffer.commands {
        let cmd = match &wgpu_cmd.command {
            Some(c) => c,
            None => continue,
        };

        match cmd {
            Command::SetPipeline(p) => {
                if let Some(veldmap_host_core::resources::Resource::RenderPipeline(pipeline)) = resources.get_resource(p.pipeline_id) {
                    rp.set_pipeline(pipeline.as_ref());
                } else {
                    log::warn!("Proxy: Pipeline {} not found", p.pipeline_id);
                }
            }
            Command::SetBindGroup(bg) => {
                let res = resources.get_resource(bg.bind_group_id);
                match res {
                    Some(veldmap_host_core::resources::Resource::BindGroup(bind_group)) => {
                        rp.set_bind_group(bg.index, bind_group.as_ref(), &bg.dynamic_offsets);
                    }
                    Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) => {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let device = resources.get_device();
                        let bg_res = device.create_bind_group(&wgpu::BindGroupDescriptor { 
                            label: Some("Proxy Auto BG Texture"), 
                            layout: &bind_group_layout, 
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
                            ] 
                        });
                        rp.set_bind_group(bg.index, &bg_res, &[]);
                    }
                    Some(veldmap_host_core::resources::Resource::Buffer(buf)) => {
                        let device = resources.get_device();
                        let uniform_bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                            label: None,
                            entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }],
                        });
                        let bg_res = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Proxy Auto BG Uniform"),
                            layout: &uniform_bg_layout,
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
                        });
                        rp.set_bind_group(bg.index, &bg_res, &[]);
                    }
                    _ => {
                        log::warn!("Proxy: Resource {} not found or is not a BindGroup/Texture/Buffer", bg.bind_group_id);
                    }
                }
            }
            Command::SetVertexBuffer(vb) => {
                if let Some(veldmap_host_core::resources::Resource::Buffer(buf)) = resources.get_resource(vb.buffer_id) {
                    let end = if vb.size > 0 { (vb.offset + vb.size).min(buf.size()) } else { buf.size() };
                    rp.set_vertex_buffer(vb.slot, buf.slice(vb.offset..end));
                }
            }
            Command::SetIndexBuffer(ib) => {
                let format = if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                if let Some(veldmap_host_core::resources::Resource::Buffer(buf)) = resources.get_resource(ib.buffer_id) {
                    let end = if ib.size > 0 { (ib.offset + ib.size).min(buf.size()) } else { buf.size() };
                    rp.set_index_buffer(buf.slice(ib.offset..end), format);
                }
            }
            Command::Draw(d) => {
                rp.draw(d.first_vertex..(d.first_vertex + d.vertex_count), d.first_instance..(d.first_instance + d.instance_count));
            }
            Command::DrawIndexed(di) => {
                rp.draw_indexed(di.first_index..(di.first_index + di.index_count), di.base_vertex, di.first_instance..(di.first_instance + di.instance_count));
            }
            Command::SetViewport(v) => {
                rp.set_viewport(v.x, v.y, v.width, v.height, v.min_depth, v.max_depth);
            }
            Command::SetScissorRect(s) => {
                rp.set_scissor_rect(s.x, s.y, s.width, s.height);
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Включаем подробные логи для отладки
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info,veldmap_host=trace,veldmap_host_gui=trace,veldmap_host_core=trace,wgpu_core=warn,wgpu_hal=warn");
    }
    env_logger::init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap GUI Host starting (DEBUG MODE)...");

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    
    let window = Arc::new(WindowBuilder::new()
        .with_title("VeldMap (Debug)")
        .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0))
        .build(&event_loop)?);

    let (tx, mut rx) = mpsc::unbounded_channel::<AppCommand>();
    let is_visible = Arc::new(AtomicBool::new(true));
    let ui_busy = Arc::new(AtomicBool::new(false));

    let flags = wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags,
        ..Default::default()
    });
    
    let surface = instance.create_surface(window.clone())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { 
        compatible_surface: Some(&surface), 
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default() 
    }).await.ok_or_else(|| anyhow::anyhow!("Compatible GPU adapter not found."))?;
    
    let info = adapter.get_info();
    log::info!("Using GPU adapter: {} ({:?}, driver: {}, backend: {:?})", info.name, info.device_type, info.driver, info.backend);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats[0];
    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    if size.width > 0 && size.height > 0 {
        surface.configure(&device, &config);
    }

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Blit Bind Group Layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(queue);
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone(), surface_format));

    // Создаем темно-серую заглушку 1x1 для бинд-группы 1001
    let white_tex_id = resources.create_texture(1, 1, 0, 8);
    resources.write_resource(white_tex_id, 0, &[30, 30, 33, 255]).unwrap();
    if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(white_tex_id) {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bg = device_arc.create_bind_group(&wgpu::BindGroupDescriptor { 
            label: Some("Default White BG"), 
            layout: &bind_group_layout, 
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
            ] 
        });
        resources.register_bind_group(1001, Arc::new(bg));
        resources.register_named_resource("active_ui_bind_group", 1001);
    }

    let mut app_texture_id: Option<u64> = None;
    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut recorded_commands: Option<veldmap_host_core::wgpu::CommandBuffer> = None;
    let mut last_size = (100u32, 100u32);
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_cursor_sent_time = std::time::Instant::now();

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone()))));
    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(tx, proxy, is_visible.clone(), resources.clone()))));

    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir).await?;

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let node = Arc::new(VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });
        log::info!("Core ready. Heartbeat started...");
        
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;
            if is_visible_clone.load(Ordering::Relaxed) {
                if let Err(e) = d_clone.call("data-browser", "render", vec![]).await {
                    log::error!("Render call failed: {}", e);
                }
            }
        }
    });

    event_loop.run(move |event: Event<()>, window_target: &EventLoopWindowTarget<()>| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(()) => {
                let mut last_draw_cmd = None;
                while let Ok(cmd) = rx.try_recv() { last_draw_cmd = Some(cmd); }

                match last_draw_cmd {
                    Some(AppCommand::Draw(id, w, h)) => {
                        let size = window.inner_size();
                        if is_visible.load(Ordering::SeqCst) && size.width > 0 && size.height > 0 && w > 0 && h > 0 {
                            if Some(id) != app_texture_id || (w, h) != last_size || app_bind_group.is_none() {
                                if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(id) {
                                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                    let device = resources.get_device();
                                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { 
                                        label: None, 
                                        layout: &bind_group_layout, 
                                        entries: &[
                                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
                                        ] 
                                    });
                                                                                                    app_texture_id = Some(id);
                                                                                                    app_bind_group = Some(bind_group.clone());
                                                                                                    let bg_id = 1001;
                                                                                                    resources.register_bind_group(bg_id, Arc::new(bind_group));
                                                                                                    resources.register_named_resource("active_ui_bind_group", bg_id);
                                                                                                    last_size = (w, h);
                                                                                                        recorded_commands = None;
                                }
                            }
                            window.request_redraw();
                        }
                    }
                    Some(AppCommand::Render { width, height, command_buffer }) => {
                        recorded_commands = Some(command_buffer);
                        last_size = (width, height);
                        app_texture_id = None;
                        app_bind_group = None;
                        window.request_redraw();
                    }
                    None => {}
                }
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let size = window.inner_size();
                if is_visible.load(Ordering::SeqCst) && size.width > 0 && size.height > 0 && config.width > 0 && config.height > 0 {
                    match surface.get_current_texture() {
                        Ok(frame) => {
                            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let mut encoder = resources.get_device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                            {
                                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { 
                                    label: None, 
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { 
                                        view: &view, 
                                        resolve_target: None, 
                                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } 
                                    })], 
                                    depth_stencil_attachment: None, ..Default::default() 
                                });

                                if let Some(cmds) = &recorded_commands {
                                    if let Err(e) = execute_render_commands(&mut rp, cmds, &resources, &bind_group_layout, &sampler) {
                                        log::error!("Render commands execution failed: {}", e);
                                    }
                                }
                            }
                            resources.get_device().poll(wgpu::Maintain::Wait);
                            queue_arc.submit(Some(encoder.finish()));
                            frame.present();
                        }
                        Err(wgpu::SurfaceError::Outdated) => {}
                        Err(e) => log::error!("Surface error: {:?}", e),
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    is_visible.store(true, Ordering::SeqCst);
                    config.width = size.width; config.height = size.height;
                    surface.configure(&device_arc, &config);
                    window.request_redraw();
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Resize(veldmap_host_core::ui::ResizeEvent { width: size.width, height: size.height, scale_factor: window.scale_factor() as f32 })) };
                    let d_clone = dispatcher.clone();
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; });
                } else {
                    is_visible.store(false, Ordering::SeqCst);
                }
            }
            Event::WindowEvent { event: WindowEvent::Occluded(occluded), .. } => {
                is_visible.store(!occluded, Ordering::SeqCst);
                if !occluded { window.request_redraw(); }
            }
            Event::WindowEvent { event: WindowEvent::Focused(focused), .. } => {
                if focused && is_visible.load(Ordering::SeqCst) { window.request_redraw(); }
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                let pressed = state == winit::event::ElementState::Pressed;
                let btn = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Click(veldmap_host_core::ui::ClickEvent { x: cursor_pos.0, y: cursor_pos.1, button: btn, pressed })) };
                let d_clone = dispatcher.clone();
                let busy_clone = ui_busy.clone();
                tokio::spawn(async move { busy_clone.store(true, Ordering::SeqCst); let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                if !ui_busy.load(Ordering::SeqCst) && last_cursor_sent_time.elapsed() >= std::time::Duration::from_millis(50) {
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::CursorMoved(veldmap_host_core::ui::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })) };
                    let d_clone = dispatcher.clone();
                    let busy_clone = ui_busy.clone();
                    busy_clone.store(true, Ordering::SeqCst);
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
                    last_cursor_sent_time = std::time::Instant::now();
                }
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 20.0, y * 20.0),
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };
                let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Scroll(veldmap_host_core::ui::ScrollEvent { delta_x: dx, delta_y: dy })) };
                let d_clone = dispatcher.clone();
                let busy_clone = ui_busy.clone();
                tokio::spawn(async move { busy_clone.store(true, Ordering::SeqCst); let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => { window_target.exit(); }
            _ => (),
        }
    })?;

    Ok(())
}