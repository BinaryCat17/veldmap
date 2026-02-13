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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Настраиваем логи: по умолчанию только предупреждения, для нашего проекта - INFO
    // Отключаем шумные iroh, wasmtime, sctk и wgpu_core (до уровня error/warn)
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn,veldmap_host=info,veldmap_host_gui=info,veldmap_host_core=info,wasm=info,host=info,iroh=error,iroh_gossip=error,wasmtime_wasi=error,wgpu_core=error,wgpu_hal=error,sctk=error");
    }
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            use std::io::Write;
            let ts = buf.timestamp();
            let level = record.level();
            let target = record.target();
            let args = record.args();

            if target == "wasm" {
                // Логи из плагинов уже содержат [имя-плагина]
                writeln!(buf, "[{} {:5}] {}", ts, level, args)
            } else if target.starts_with("veldmap") {
                // Все наши внутренние модули теперь просто [host]
                writeln!(buf, "[{} {:5}] [host] {}", ts, level, args)
            } else {
                // Внешние библиотеки
                writeln!(buf, "[{} {:5}] <{}> {}", ts, level, target, args)
            }
        })
        .init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap GUI Host starting...");

    let mut window_width = 1024.0;
    let mut window_height = 768.0;
    let mut window_title = "VeldMap".to_string();
    let mut ui_scale = 1.0;

    // Read core.json for window settings
    let core_config_path = std::path::Path::new(&config_dir).join("core.json");
    if let Ok(config_str) = std::fs::read_to_string(core_config_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&config_str) {
            if let Some(w) = v["window"]["width"].as_f64() { window_width = w; }
            if let Some(h) = v["window"]["height"].as_f64() { window_height = h; }
            if let Some(t) = v["window"]["title"].as_str() { window_title = t.to_string(); }
            if let Some(s) = v["window"]["ui_scale"].as_f64() { ui_scale = s; }
        }
    }

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    
    let window = Arc::new(WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(winit::dpi::LogicalSize::new(window_width, window_height))
        .build(&event_loop)?);

    let (tx, mut rx) = mpsc::unbounded_channel::<AppCommand>();
    let is_visible = Arc::new(AtomicBool::new(true));
    let ui_busy = Arc::new(AtomicBool::new(false));

    let flags = wgpu::InstanceFlags::default() 
        | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER
        | wgpu::InstanceFlags::DEBUG
        | wgpu::InstanceFlags::VALIDATION;
    
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

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(std::sync::Mutex::new(queue));
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone(), surface_format));

    // Инициализируем Blit Pipeline для вывода текстур плагинов
    let blit_shader = device_arc.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));
    let blit_pipeline_layout = device_arc.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Blit Pipeline Layout"),
        bind_group_layouts: &[&resources.get_ui_layout()],
        push_constant_ranges: &[],
    });
    let blit_pipeline = device_arc.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blit Pipeline"),
        layout: Some(&blit_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &blit_shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &blit_shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let bind_group_layout = resources.get_ui_layout();
    let sampler = resources.get_ui_sampler();

    // Создаем темно-серую заглушку 1x1 для бинд-группы 1001
    let white_tex_id = resources.create_texture(1, 1, 0, 8, false);
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
    let mut last_size = (100u32, 100u32);
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_cursor_sent_time = std::time::Instant::now();

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone(), dispatcher.tasks.clone()))));
    dispatcher.register_service("wgpu".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::gpu_service::GpuService::new(resources.clone()))));
    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(tx, proxy, is_visible.clone(), resources.clone()))));

    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir).await?;

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let node = Arc::new(VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });
        log::info!("Core ready. Heartbeat started...");
        
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
        loop {
            interval.tick().await;
            
            // Poll all WASM tasks (Fibers)
            let _ = d_clone.poll_all_tasks().await;

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
                                }
                            }
                            window.request_redraw();
                        }
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
                                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.07, a: 1.0 }), store: wgpu::StoreOp::Store } 
                                    })], 
                                    depth_stencil_attachment: None, ..Default::default() 
                                });

                                if let Some(bg) = &app_bind_group {
                                    rp.set_pipeline(&blit_pipeline);
                                    rp.set_bind_group(0, bg, &[]);
                                    rp.draw(0..3, 0..1);
                                }
                            }
                            {
                                let mut q = queue_arc.lock().unwrap();
                                q.submit(Some(encoder.finish()));
                            }
                            frame.present();
                        }
                        Err(wgpu::SurfaceError::Outdated) => {
                            surface.configure(&device_arc, &config);
                        }
                        Err(wgpu::SurfaceError::Lost) => {
                            surface.configure(&device_arc, &config);
                        }
                        Err(e) => log::error!("Surface error: {:?}", e),
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                let scale_factor = window.scale_factor() * ui_scale;
                if size.width > 0 && size.height > 0 {
                    is_visible.store(true, Ordering::SeqCst);
                    config.width = size.width; config.height = size.height;
                    surface.configure(&device_arc, &config);
                    window.request_redraw();
                    let ev = veldmap_host_core::app::UiEvent { event: Some(veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent { width: size.width, height: size.height, scale_factor: scale_factor as f32 })) };
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
                let ev = veldmap_host_core::app::UiEvent { event: Some(veldmap_host_core::app::ui_event::Event::Click(veldmap_host_core::app::ClickEvent { x: cursor_pos.0, y: cursor_pos.1, button: btn, pressed })) };
                let d_clone = dispatcher.clone();
                let busy_clone = ui_busy.clone();
                tokio::spawn(async move { busy_clone.store(true, Ordering::SeqCst); let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                if !ui_busy.load(Ordering::SeqCst) && last_cursor_sent_time.elapsed() >= std::time::Duration::from_millis(50) {
                    let ev = veldmap_host_core::app::UiEvent { event: Some(veldmap_host_core::app::ui_event::Event::CursorMoved(veldmap_host_core::app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })) };
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
                let ev = veldmap_host_core::app::UiEvent { event: Some(veldmap_host_core::app::ui_event::Event::Scroll(veldmap_host_core::app::ScrollEvent { delta_x: dx, delta_y: dy })) };
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