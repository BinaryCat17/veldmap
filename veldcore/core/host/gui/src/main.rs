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
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn,veldmap_host=info,veldmap_host_gui=info,veldmap_host_core=info,wasm=info,host=info,iroh=error,iroh_gossip=error,wasmtime_wasi=error,wgpu_core=info,wgpu_hal=info,sctk=error,egl=info,gles=info");
    }
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            use std::io::Write;
            let ts = buf.timestamp();
            let level = record.level();
            let target = record.target();
            let args = record.args();

            if target == "wasm" {
                writeln!(buf, "[{} {:5}] {}", ts, level, args)
            } else if target.starts_with("veldmap") {
                writeln!(buf, "[{} {:5}] [host] {}", ts, level, args)
            } else {
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
    let mut vsync = true;
    let mut fps_limit = 60;
    let mut auto_fps = false;

    let core_config_path = std::path::Path::new(&config_dir).join("core.json");
    if let Ok(config_str) = std::fs::read_to_string(core_config_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&config_str) {
            if let Some(w) = v["window"]["width"].as_f64() { window_width = w; }
            if let Some(h) = v["window"]["height"].as_f64() { window_height = h; }
            if let Some(t) = v["window"]["title"].as_str() { window_title = t.to_string(); }
            if let Some(s) = v["window"]["ui_scale"].as_f64() { ui_scale = s; }
            
            if let Some(fps_val) = v["window"]["fps"].as_str() {
                if fps_val == "vsync" { 
                    vsync = true; 
                    auto_fps = false;
                } else if fps_val == "auto" {
                    vsync = true;
                    auto_fps = true;
                }
            } else if let Some(fps_num) = v["window"]["fps"].as_i64() {
                vsync = false;
                auto_fps = false;
                fps_limit = fps_num as i32;
            }
        }
    }

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    
    let window = Arc::new(WindowBuilder::new()
        .with_title(window_title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(window_width, window_height))
        .build(&event_loop)?);

    // ОПРЕДЕЛЯЕМ ЧАСТОТУ МОНИТОРА
    let monitor_fps = window.current_monitor()
        .and_then(|m| m.refresh_rate_millihertz())
        .map(|mhz| (mhz as f32 / 1000.0).round() as i32)
        .unwrap_or(60);
    
    log::info!("Detected Monitor Refresh Rate: {} Hz", monitor_fps);
    
    if fps_limit == 60 { // Если в конфиге не задано иное, используем частоту монитора
        fps_limit = monitor_fps;
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<AppCommand>();
    let is_visible = Arc::new(AtomicBool::new(true));
    let frame_pending = Arc::new(AtomicBool::new(false));
    let frame_wake = Arc::new(tokio::sync::Notify::new());

    let flags = wgpu::InstanceFlags::default() 
        | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    
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
    log::info!("Selected GPU: {} ({:?}, {:?})", info.name, info.device_type, info.backend);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(std::sync::Mutex::new(queue));

    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats[0];
    
    let present_mode = if vsync {
        if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox // Better VSync: no tearing, high performance
        } else {
            wgpu::PresentMode::Fifo
        }
    } else {
        if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        }
    };
    
    log::info!("Selected Present Mode: {:?}", present_mode);

    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    
    surface.configure(&device_arc, &config);

    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone(), surface_format));

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
        depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
    });

    let bind_group_layout = resources.get_ui_layout();
    let sampler = resources.get_ui_sampler();

    let render_queue = Arc::new(std::sync::Mutex::new(Vec::<veldmap_host_core::wgpu::Submit>::new()));

    let mut app_texture_id: Option<u64> = None;
    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut last_size = (100u32, 100u32);
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_cursor_sent_time = std::time::Instant::now();

    let mut frame_count = 0;
    let mut last_fps_update = std::time::Instant::now();

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone(), dispatcher.tasks.clone()))));
    dispatcher.register_service("wgpu".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::gpu_service::GpuService::new(resources.clone(), render_queue.clone()))));
    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(tx, proxy, is_visible.clone(), resources.clone()))));

    let last_interaction_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let last_render_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir).await?;

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    let frame_pending_clone = frame_pending.clone();
    let last_int_clone = last_interaction_time.clone();
    let last_rend_clone = last_render_time.clone();
    let frame_wake_clone = frame_wake.clone();

    // 1. ЦИКЛ ОБРАБОТКИ ЗАДАЧ (POLLING) - Максимальная отзывчивость
    let d_tasks = dispatcher.clone();
    tokio::spawn(async move {
        loop {
            let _ = d_tasks.poll_all_tasks().await;
            
            // Адаптивная частота поллинга: 1мс если есть задачи, 10мс если нет
            let has_tasks = {
                let tasks = d_tasks.tasks.lock().unwrap();
                !tasks.is_empty()
            };
            
            if has_tasks {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    });

    // 2. ЦИКЛ ОТРИСОВКИ (FRAME PACING) - Энергоэффективность
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let node = Arc::new(VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });
        log::info!("Core ready. Render loop started (VSync: {}, FPS Limit: {}, Auto: {})...", vsync, fps_limit, auto_fps);
        
        let mut last_frame_time = std::time::Instant::now();
        loop {
            let now = std::time::Instant::now();
            let dt = now.duration_since(last_frame_time).as_secs_f32();
            
            // Если предыдущий кадр еще не обработан WASM-модулем - ждем
            if frame_pending_clone.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                continue;
            }

            // АДАПТИВНЫЙ FPS
            let mut current_fps_limit = fps_limit;
            if auto_fps {
                let last_int = *last_int_clone.lock().unwrap();
                let last_rend = *last_rend_clone.lock().unwrap();
                let idle_time = now.duration_since(last_int).as_secs_f32();
                let render_idle_time = now.duration_since(last_rend).as_secs_f32();

                // Если нет ввода > 0.5 сек и нет отрисовок > 0.3 сек - снижаем до 5 FPS
                if idle_time > 0.5 && render_idle_time > 0.3 {
                    current_fps_limit = 5;
                } else {
                    current_fps_limit = fps_limit; // Используем частоту монитора или лимит из конфига
                }
            }

            // Ограничиваем FPS, если мы не в режиме чистого VSync без лимита
            // Или если текущий лимит (например, 5 FPS) ниже частоты монитора
            // Всегда соблюдаем лимит FPS, чтобы не нагружать CPU лишними кадрами,
            // которые монитор всё равно не успеет показать.
            let target_dt = 1.0 / current_fps_limit.max(1) as f32;
            if dt < target_dt {
                // Ждем либо следующего тика, либо уведомления о вводе пользователя
                let sleep_duration = std::time::Duration::from_secs_f32(target_dt - dt);
                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {},
                    _ = frame_wake_clone.notified() => {
                        // Проснулись по вводу - сбрасываем таймер кадра, чтобы выдать его немедленно
                    }
                }
            }
            
            last_frame_time = std::time::Instant::now();

            if is_visible_clone.load(Ordering::Relaxed) {
                frame_pending_clone.store(true, Ordering::Release);
                let ev = veldmap_host_core::app::UiEvent { 
                    surface_handle: Some(veldmap_host_core::core::ResourceHandle { 
                        id: veldmap_host_core::SURFACE_ID, 
                        ..Default::default() 
                    }),
                    event: Some(veldmap_host_core::app::ui_event::Event::Frame(veldmap_host_core::app::FrameEvent { 
                        dt,
                    })) 
                };
                
                // Вызываем RPC и СРАЗУ сбрасываем флаг ожидания после возврата
                let call_result = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec(), 0).await;
                frame_pending_clone.store(false, Ordering::Release);
                
                if let Err(e) = call_result {
                    log::error!("Frame event failed: {}", e);
                }
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    });

    event_loop.run(move |event: Event<()>, window_target: &EventLoopWindowTarget<()>| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(()) => {
                let mut last_draw_cmd = None;
                while let Ok(cmd) = rx.try_recv() { last_draw_cmd = Some(cmd); }
                if let Some(AppCommand::Draw(id, w, h)) = last_draw_cmd {
                    *last_render_time.lock().unwrap() = std::time::Instant::now();
                    if is_visible.load(Ordering::SeqCst) && w > 0 && h > 0 {
                        if id == veldmap_host_core::SURFACE_ID {
                            // Direct surface rendering
                            app_texture_id = Some(veldmap_host_core::SURFACE_ID);
                            app_bind_group = None;
                        } else if Some(id) != app_texture_id || (w, h) != last_size {
                            if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(id, 0) {
                                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let bind_group = resources.get_device().create_bind_group(&wgpu::BindGroupDescriptor { 
                                    label: None, layout: &bind_group_layout, entries: &[
                                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
                                    ] 
                                });
                                app_texture_id = Some(id);
                                app_bind_group = Some(bind_group.clone());
                                resources.register_bind_group(1001, Arc::new(bind_group), 0);
                                resources.register_named_resource("active_ui_bind_group", 1001);
                                last_size = (w, h);
                            }
                        }
                        window.request_redraw();
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let size = window.inner_size();
                if is_visible.load(Ordering::SeqCst) && size.width > 0 && size.height > 0 && config.width > 0 && config.height > 0 {
                    match surface.get_current_texture() {
                        Ok(frame) => {
                            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let mut encoder = resources.get_device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                            
                            let (target_w, target_height) = (config.width, config.height);

                            // Process deferred render commands from plugins
                            let deferred_commands: Vec<_> = {
                                let mut q = render_queue.lock().unwrap();
                                std::mem::take(&mut *q)
                            };

                            // Split commands into surface-targeted and texture-targeted
                            let mut surface_cmds = Vec::new();
                            let mut texture_cmds = Vec::new();
                            for req in deferred_commands {
                                if req.target_texture_view_id == veldmap_host_core::SURFACE_ID {
                                    surface_cmds.push(req);
                                } else {
                                    texture_cmds.push(req);
                                }
                            }

                            // 1. Process texture-targeted commands first (off-screen)
                            for req in &texture_cmds {
                                let target_view_arc: Arc<wgpu::TextureView> = match resources.get_resource(req.target_texture_view_id, req.instance_id) {
                                    Some(veldmap_host_core::resources::Resource::TextureView(v)) => v,
                                    Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) => {
                                        Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()))
                                    },
                                    _ => continue,
                                };

                                let clear = req.clear_color.clone().unwrap_or_default();
                                {
                                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: Some("Plugin-Texture-Pass"),
                                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                            view: &target_view_arc,
                                            resolve_target: None,
                                            ops: wgpu::Operations {
                                                load: wgpu::LoadOp::Clear(wgpu::Color { r: clear.r as f64, g: clear.g as f64, b: clear.b as f64, a: clear.a as f64 }),
                                                store: wgpu::StoreOp::Store,
                                            },
                                        })],
                                        depth_stencil_attachment: None, ..Default::default()
                                    });

                                    if let Some(cb) = &req.command_buffer {
                                        let _ = veldmap_host_core::gpu_service::execute_render_commands(&mut rp, cb, &resources, 2048, 2048, req.instance_id);
                                    }
                                }
                            }

                            // 2. MAIN PASS: Surface rendering
                            {
                                let bg_color = wgpu::Color { r: 0.05, g: 0.05, b: 0.07, a: 1.0 };
                                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { 
                                    label: Some("Main-Surface-Pass"), 
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { 
                                        view: &view, resolve_target: None, 
                                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg_color), store: wgpu::StoreOp::Store } 
                                    })], 
                                    depth_stencil_attachment: None, ..Default::default() 
                                });

                                // Draw Direct Surface Commands from plugins
                                for req in &surface_cmds {
                                    if let Some(cb) = &req.command_buffer {
                                        let _ = veldmap_host_core::gpu_service::execute_render_commands(&mut rp, cb, &resources, target_w, target_height, req.instance_id);
                                    }
                                }

                                // Draw Host-level blit (if any)
                                if let Some(bg) = &app_bind_group {
                                    rp.set_pipeline(&blit_pipeline);
                                    rp.set_bind_group(0, bg, &[]);
                                    rp.draw(0..3, 0..1);
                                }
                            }
                            queue_arc.lock().unwrap().submit(Some(encoder.finish()));
                            frame.present();

                            frame_count += 1;
                            let now = std::time::Instant::now();
                            let elapsed = now.duration_since(last_fps_update);
                            if elapsed >= std::time::Duration::from_secs(1) {
                                window.set_title(&format!("{} - {:.1} FPS", window_title, frame_count as f64 / elapsed.as_secs_f64()));
                                frame_count = 0;
                                last_fps_update = now;
                            }
                        }
                        Err(e) => {
                            log::error!("Surface error: {:?}", e);
                            surface.configure(&device_arc, &config);
                            frame_pending.store(false, Ordering::Release);
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    config.width = size.width; config.height = size.height;
                    surface.configure(&device_arc, &config);
                    let ev = veldmap_host_core::app::UiEvent { 
                        surface_handle: Some(veldmap_host_core::core::ResourceHandle { 
                            id: veldmap_host_core::SURFACE_ID, 
                            ..Default::default() 
                        }),
                        event: Some(veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent { 
                            width: size.width, 
                            height: size.height, 
                            scale_factor: (window.scale_factor() * ui_scale) as f32,
                        })) 
                    };
                    let d = dispatcher.clone();
                    tokio::spawn(async move { let _ = d.call("data-browser", "handle_ui_event", ev.encode_to_vec(), 0).await; });
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => { window_target.exit(); }
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                *last_interaction_time.lock().unwrap() = std::time::Instant::now();
                frame_wake.notify_one();
                let btn = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                let ev = veldmap_host_core::app::UiEvent { 
                    surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: veldmap_host_core::SURFACE_ID, ..Default::default() }),
                    event: Some(veldmap_host_core::app::ui_event::Event::Click(veldmap_host_core::app::ClickEvent { x: cursor_pos.0, y: cursor_pos.1, button: btn, pressed: state == winit::event::ElementState::Pressed })) 
                };
                let d = dispatcher.clone();
                                    tokio::spawn(async move { let _ = d.call("data-browser", "handle_ui_event", ev.encode_to_vec(), 0).await; });
                
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                *last_interaction_time.lock().unwrap() = std::time::Instant::now();
                frame_wake.notify_one();
                cursor_pos = (position.x as f32, position.y as f32);
                if last_cursor_sent_time.elapsed() >= std::time::Duration::from_millis(16) {
                    let ev = veldmap_host_core::app::UiEvent { 
                        surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: veldmap_host_core::SURFACE_ID, ..Default::default() }),
                        event: Some(veldmap_host_core::app::ui_event::Event::CursorMoved(veldmap_host_core::app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })) 
                    };
                    let d = dispatcher.clone();
                    tokio::spawn(async move { let _ = d.call("data-browser", "handle_ui_event", ev.encode_to_vec(), 0).await; });
                    last_cursor_sent_time = std::time::Instant::now();
                }
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                *last_interaction_time.lock().unwrap() = std::time::Instant::now();
                frame_wake.notify_one();
                let (pdx, pdy) = match delta { winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0), winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32) };
                let ev = veldmap_host_core::app::UiEvent { 
                    surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: veldmap_host_core::SURFACE_ID, ..Default::default() }),
                    event: Some(veldmap_host_core::app::ui_event::Event::Scroll(veldmap_host_core::app::ScrollEvent { delta_x: pdx, delta_y: pdy })) 
                };
                let d = dispatcher.clone();
                tokio::spawn(async move { let _ = d.call("data-browser", "handle_ui_event", ev.encode_to_vec(), 0).await; });
            }
            _ => (),
        }
    })?;
    Ok(())
}
