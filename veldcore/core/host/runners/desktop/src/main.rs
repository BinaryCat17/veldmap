#![recursion_limit = "512"]
use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    plugin_module,
};
use veldmap_host_system::SystemService;
use veldmap_host_compute::ComputeService;

use crate::app_service::{AppCommand, AppService};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use std::sync::{Arc, Mutex};
use std::io::Write;
use prost::Message;

mod app_service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- 1. ИНИЦИАЛИЗАЦИЯ ЛОГИРОВАНИЯ ---
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("host.log")
        .ok();
    let log_file = log_file.map(|f| std::sync::Mutex::new(f));

    env_logger::Builder::from_default_env()
        .format(move |buf, record| {
            let log_line = format!(
                "[{}] <{}> {}\n",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ"),
                record.target(),
                record.args()
            );

            if let Some(file) = &log_file {
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(log_line.as_bytes());
                }
            }

            write!(buf, "{}", log_line)
        })
        .init();

    log::info!("VeldMap GUI Host starting...");

    // --- 2. ПАРСИНГ АРГУМЕНТОВ ---
    let args: Vec<String> = std::env::args().collect();
    let config_dir = args.iter().position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "config".to_string());

    // --- 3. ИНИЦИАЛИЗАЦИЯ ГРАФИКИ (WGPU) ---
    let mut window_width = 1024.0;
    let mut window_height = 768.0;
    let mut window_title = "VeldMap".to_string();

    let core_config_path = std::path::Path::new(&config_dir).join("core.json");
    if let Ok(config_str) = std::fs::read_to_string(core_config_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&config_str) {
            if let Some(w) = v["window"]["width"].as_f64() { window_width = w; }
            if let Some(h) = v["window"]["height"].as_f64() { window_height = h; }
            if let Some(t) = v["window"]["title"].as_str() { window_title = t.to_string(); }
        }
    }

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let window = Arc::new(WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(winit::dpi::LogicalSize::new(window_width, window_height))
        .build(&event_loop)?);

    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window.clone())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }).await.map_err(|e| anyhow::anyhow!("Adapter error: {}", e))?;

    log::info!("Selected GPU: {:?}", adapter.get_info().name);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        ..Default::default()
    }).await?;

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(Mutex::new(queue));

    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats.iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);

    log::info!("Available Present Modes: {:?}", caps.present_modes);
    let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    };
    log::info!("Selected Present Mode: {:?}", present_mode);

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: window.inner_size().width,
        height: window.inner_size().height,
        present_mode,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device_arc, &config);

    // --- 4. ИНИЦИАЛИЗАЦИЯ ЯДРА И СЕРВИСОВ ---
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone(), surface_format));

    let blit_shader = device_arc.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));
    let blit_pipeline_layout = device_arc.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Blit Pipeline Layout"),
        bind_group_layouts: &[&veldmap_host_compute::get_ui_layout(&device_arc)],
        immediate_size: 0,
    });
    let blit_pipeline = device_arc.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blit Pipeline"),
        layout: Some(&blit_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &blit_shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &blit_shader, entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview_mask: None, cache: None,
    });

    let sampler = device_arc.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let render_queue = Arc::new(std::sync::Mutex::new(Vec::<veldmap_host_core::compute::Submit>::new()));

    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut app_texture_id: Option<u64> = None;
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_cursor_sent_time = std::time::Instant::now();

    // Iroh Node Setup
    let mut rng = rand::rng();
    
    let secret_key = iroh::SecretKey::generate(&mut rng);
    let endpoint = iroh::Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![b"veldmap/rpc/1".to_vec()])
        .bind()
        .await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));

    let actual_fps = Arc::new(std::sync::Mutex::new(60.0f32));
    let last_render_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

    let system_service = Arc::new(SystemService::new(resources.clone(), dispatcher.tasks.clone()));
    let compute_service = Arc::new(ComputeService::new(resources.clone(), render_queue.clone()));

    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(system_service.clone()));
    dispatcher.register_service("compute".to_string(), ServiceLocation::Native(compute_service));

    // Register Modular Services
    dispatcher.register_service("fs".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_fs::FsService::new(resources.clone()))));
    dispatcher.register_service("network".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_network::NetworkService::new(dispatcher.tasks.clone()))));
    dispatcher.register_service("image".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_image::ImageService::new(resources.clone(), dispatcher.tasks.clone()))));
    
    let is_visible = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppCommand>();
    let proxy = event_loop.create_proxy();
    let frame_wake = Arc::new(tokio::sync::Notify::new());

    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(
        tx, 
        proxy.clone(), 
        is_visible.clone(), 
        resources.clone(),
        last_render_time.clone(),
        frame_wake.clone(),
    ))));

    let last_interaction_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let event_queue = Arc::new(std::sync::Mutex::new(Vec::<veldmap_host_core::app::UiEvent>::new()));

    let sys_clone = system_service.clone();
    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir, move |id, cfg| {
        sys_clone.register_config(id, cfg);
    }).await?;

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    let frame_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let frame_pending_clone = frame_pending.clone();
    let last_int_clone = last_interaction_time.clone();
    let last_rend_clone = last_render_time.clone();
    let frame_wake_clone = frame_wake.clone();
    let event_queue_clone = event_queue.clone();

    // 1. ЦИКЛ ОБРАБОТКИ ЗАДАЧ (POLLING)
    tokio::spawn(async move {
        loop {
            let has_tasks = {
                let tasks = d_clone.tasks.lock().unwrap();
                !tasks.is_empty()
            };

            if has_tasks || is_visible_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = d_clone.poll_all_tasks().await;
                
                let needs_frame = {
                    let eq = event_queue_clone.lock().unwrap();
                    let last_int = last_int_clone.lock().unwrap();
                    let last_rend = last_rend_clone.lock().unwrap();
                    
                    !eq.is_empty() || last_int.elapsed().as_millis() < 500 || last_rend.elapsed().as_millis() > 1000
                };

                if needs_frame && !frame_pending_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    frame_pending_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    frame_wake_clone.notify_one();
                }

                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    });

    // 2. ЦИКЛ ОТРИСОВКИ (FRAME PACING)
    let mut last_render_finish = std::time::Instant::now();
    let frame_wake_render = frame_wake.clone();
    let window_render = window.clone();
    let frame_pending_render = frame_pending.clone();
    
    let dispatcher_render = dispatcher.clone();
    let event_queue_render = event_queue.clone();
    let actual_fps_render = actual_fps.clone();

    tokio::spawn(async move {
        loop {
            frame_wake_render.notified().await;
            
            let start_redraw = std::time::Instant::now();
            let mut events = {
                let mut eq = event_queue_render.lock().unwrap();
                std::mem::take(&mut *eq)
            };

            let dt = start_redraw.duration_since(last_render_finish).as_secs_f32();
            let fps = *actual_fps_render.lock().unwrap();
            
            events.push(veldmap_host_core::app::UiEvent {
                event: Some(veldmap_host_core::app::ui_event::Event::Frame(veldmap_host_core::app::FrameEvent {
                    dt,
                    actual_fps: fps,
                    monitor_fps: 60,
                    surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: 0, size: 0, content_hash: Vec::new() }),
                })),
                ..Default::default()
            });

            for ev in events {
                let payload = ev.encode_to_vec();
                let _ = dispatcher_render.call("data-browser", "handle_ui_event", payload, 0).await;
            }

            window_render.request_redraw();
            frame_pending_render.store(false, std::sync::atomic::Ordering::SeqCst);
            last_render_finish = std::time::Instant::now();
        }
    });

    log::info!("Core ready. Render loop started...");
    log::info!("Veldmap Iroh Node listening. Node ID: {}", endpoint.id());

    event_loop.run(move |event, window_target| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(()) => {
                let mut last_draw_cmd = None;
                while let Ok(cmd) = rx.try_recv() {
                    last_draw_cmd = Some(cmd);
                }
                if let Some(AppCommand::Draw(id)) = last_draw_cmd {
                    if is_visible.load(std::sync::atomic::Ordering::SeqCst) {
                        if id == veldmap_host_core::SURFACE_ID {
                            app_texture_id = Some(veldmap_host_core::SURFACE_ID);
                            app_bind_group = None;
                        } else if Some(id) != app_texture_id {
                            if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(id, 0) {
                                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let bind_group = device_arc.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some("App Bind Group"),
                                    layout: &veldmap_host_compute::get_ui_layout(&device_arc),
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(&view),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::Sampler(&sampler),
                                        },
                                    ],
                                });
                                app_texture_id = Some(id);
                                app_bind_group = Some(bind_group);
                            }
                        }
                        window.request_redraw();
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => window_target.exit(),
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                config.width = size.width.max(1);
                config.height = size.height.max(1);
                surface.configure(&device_arc, &config);
                window.request_redraw();
                let ev = veldmap_host_core::app::UiEvent {
                    event: Some(veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent {
                        width: config.width,
                        height: config.height,
                        scale_factor: window.scale_factor() as f32,
                        surface_handle: Some(veldmap_host_core::core::ResourceHandle {
                            id: 0,
                            size: 0,
                            content_hash: Vec::new(),
                        }),
                    })),
                    ..Default::default()
                };
                event_queue.lock().unwrap().push(ev);
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let start_redraw = std::time::Instant::now();
                let frame = match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(e) => { log::error!("Surface error: {}", e); return; }
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device_arc.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                let target_w = config.width;
                let target_h = config.height;

                // Offscreen pass
                let mut surface_cmds = Vec::new();
                {
                    let mut queue = render_queue.lock().unwrap();
                    let q = std::mem::take(&mut *queue);
                    for req in q {
                        if req.target_texture_view_id == 0 {
                            surface_cmds.push(req);
                        } else {
                            if let Some(veldmap_host_core::resources::Resource::TextureView(target_view)) = resources.get_resource(req.target_texture_view_id, req.instance_id) {
                                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("Plugin Offscreen RP"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: &target_view, resolve_target: None,
                                        ops: wgpu::Operations { 
                                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), 
                                            store: wgpu::StoreOp::Store 
                                        },
                                        depth_slice: None,
                                    })],
                                    depth_stencil_attachment: None, ..Default::default()
                                });

                                if let Some(cb) = &req.command_buffer {
                                    let _ = veldmap_host_compute::execute_render_commands(&mut rp, cb, &resources, 2048, 2048, req.instance_id);
                                }
                            }
                        }
                    }
                }

                // Surface pass
                {
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Main Surface RP"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view, resolve_target: None,
                            ops: wgpu::Operations { 
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }), 
                                store: wgpu::StoreOp::Store 
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None, ..Default::default()
                    });

                    for req in &surface_cmds {
                        if let Some(cb) = &req.command_buffer {
                            let _ = veldmap_host_compute::execute_render_commands(&mut rp, cb, &resources, target_w, target_h, req.instance_id);
                        }
                    }

                    if let Some(bg) = &app_bind_group {
                        rp.set_pipeline(&blit_pipeline);
                        rp.set_bind_group(0, bg, &[]);
                        rp.draw(0..3, 0..1);
                    }
                }

                queue_arc.lock().unwrap().submit(Some(encoder.finish()));
                frame.present();

                let total_time = start_redraw.elapsed();
                if total_time.as_micros() > 0 {
                    let mut fps_lock = actual_fps.lock().unwrap();
                    *fps_lock = 1.0 / total_time.as_secs_f32();
                }
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                let mut last_int = last_interaction_time.lock().unwrap();
                *last_int = std::time::Instant::now();
                
                cursor_pos = (position.x as f32, position.y as f32);
                if last_cursor_sent_time.elapsed().as_millis() > 16 {
                    let ev = veldmap_host_core::app::UiEvent { 
                        event: Some(veldmap_host_core::app::ui_event::Event::CursorMoved(veldmap_host_core::app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })),
                        ..Default::default()
                    };
                    event_queue.lock().unwrap().push(ev);
                    last_cursor_sent_time = std::time::Instant::now();
                    frame_wake.notify_one();
                }
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state: button_state, button, .. }, .. } => {
                let mut last_int = last_interaction_time.lock().unwrap();
                *last_int = std::time::Instant::now();
                frame_wake.notify_one();

                let b_idx = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                let ev = veldmap_host_core::app::UiEvent { 
                    event: Some(veldmap_host_core::app::ui_event::Event::Click(veldmap_host_core::app::ClickEvent { 
                        button: b_idx, 
                        pressed: button_state == winit::event::ElementState::Pressed,
                        x: cursor_pos.0,
                        y: cursor_pos.1
                    })),
                    ..Default::default()
                };
                event_queue.lock().unwrap().push(ev);
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                let mut last_int = last_interaction_time.lock().unwrap();
                *last_int = std::time::Instant::now();
                frame_wake.notify_one();
                let (pdx, pdy) = match delta { winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0), winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32) };
                let ev = veldmap_host_core::app::UiEvent { 
                    event: Some(veldmap_host_core::app::ui_event::Event::Scroll(veldmap_host_core::app::ScrollEvent { delta_x: pdx, delta_y: pdy })),
                    ..Default::default()
                };
                event_queue.lock().unwrap().push(ev);
            }
            _ => (),
        }
    })?;
    Ok(())
}
