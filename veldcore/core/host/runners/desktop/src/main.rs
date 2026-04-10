#![recursion_limit = "512"]
use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    plugin_module,
    window::PluginWindows,
};
use veldmap_host_system::SystemService;
use veldmap_host_compute::ComputeService;
use compositor::Compositor;

use crate::app_service::{AppCommand, AppService};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::io::Write;
use prost::Message;

mod app_service;
mod compositor;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- 1. ИНИЦИАЛИЗАЦИЯ ЛОГИРОВАНИЯ ---
    // Очищаем лог файл при старте
    let _ = std::fs::remove_file("host.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("host.log")
        .ok();
    let log_file: Option<Arc<Mutex<std::fs::File>>> = log_file.map(|f| Arc::new(Mutex::new(f)));

    // Настраиваем логирование
    // В файл пишем ВСЁ (trace и выше)
    // В консоль только veldmap info+ и warn/error от других
    let file_log = log_file.clone();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("veldmap=trace,veldmap_vlog=trace,info"))
        .format(move |buf, record| {
            let log_line = format!(
                "[{}] <{}> {}\n",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ"),
                record.target(),
                record.args()
            );

            // Пишем в файл всё
            if let Some(file) = &file_log {
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(log_line.as_bytes());
                }
            }

            // В консоль: veldmap info и warn/error от других крейтов
            // Debug/trace только в файл
            let is_veldmap = record.target().starts_with("veldmap");
            let is_info = record.level() == log::Level::Info;
            let is_warn_or_error = record.level() == log::Level::Warn || record.level() == log::Level::Error;
            
            if (is_veldmap && is_info) || is_warn_or_error {
                write!(buf, "{}", log_line)
            } else {
                Ok(())
            }
        })
        .init();

    // --- 2. ЗАГРУЗКА КОНФИГА CORE ---
    let args: Vec<String> = std::env::args().collect();
    let config_dir = args.iter().position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "config".to_string());
    
    // Загружаем core.json для получения флагов логирования
    let core_config: veldmap_host_core::CoreConfig = 
        veldmap_host_core::load_config_with_path::<veldmap_host_core::CoreConfig, _>(&format!("{}/core.json", config_dir))
            .unwrap_or_default();
    
    // Инициализируем флаги логирования
    veldmap_host_core::logging::init_logging(core_config.log_flags);
    
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "VeldMap GUI Host starting...");
    veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Log flags: 0b{:b}", core_config.log_flags);

    // --- 4. СКАНИРОВАНИЕ КОНФИГОВ ПЛАГИНОВ ---
    // Сканируем конфиги плагинов до создания окна
    let mut plugin_windows = veldmap_host_core::plugin_module::scan_window_configs(&config_dir)?;
    
    // Получаем параметры окна из первого плагина с window config
    let (window_width, window_height, window_title, _ui_scale) = plugin_windows
        .first()
        .map(|(name, cfg)| {
            veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Using window config from plugin '{}'", name);
            (cfg.width as f64, cfg.height as f64, cfg.title.clone(), cfg.ui_scale)
        })
        .unwrap_or_else(|| {
            veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "No plugin window config found, using defaults");
            (1024.0, 768.0, "VeldMap".to_string(), 1.0f32)
        });

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let window = Arc::new(WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(winit::dpi::LogicalSize::new(window_width, window_height))
        .build(&event_loop)?);

    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Creating wgpu instance (Vulkan only)...");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::all(),
        ..Default::default()
    });
    
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Creating surface...");
    let surface = instance.create_surface(window.clone())?;
    
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Enumerating Vulkan adapters...");
    let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN).await;
    for (i, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Adapter {}: {:?} (vendor: 0x{:04X}, device: 0x{:04X})", 
            i, info.name, info.vendor, info.device);
    }
    
    // Выбираем реальный GPU (не llvmpipe/software)
    let mut adapter = None;
    for a in adapters {
        let info = a.get_info();
        // Исключаем llvmpipe и software renderers
        if !info.name.to_lowercase().contains("llvmpipe") && 
           !info.name.to_lowercase().contains("software") &&
           info.vendor != 0x1414 { // Microsoft (software adapters)
            adapter = Some(a);
            break;
        }
    }
    
    // Fallback: если не нашли дискретный GPU, пробуем request_adapter
    let adapter = match adapter {
        Some(a) => a,
        None => {
            veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "No discrete GPU found, trying fallback...");
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: true,
            }).await.map_err(|e| anyhow::anyhow!("Adapter error: {}", e))?
        }
    };

    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Selected GPU: {:?}", adapter.get_info().name);

    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Requesting device...");
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        ..Default::default()
    }).await?;

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(Mutex::new(queue));

    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Getting surface capabilities...");
    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats.iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);

    veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Available Present Modes: {:?}", caps.present_modes);
    let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    };
    veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Selected Present Mode: {:?}", present_mode);

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
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Initial surface configure: {}x{}", config.width, config.height);
    surface.configure(&device_arc, &config);

    // --- 4. ИНИЦИАЛИЗАЦИЯ ЯДРА И СЕРВИСОВ ---
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone(), surface_format));
    
    // Initialize compositor for final UI composition
    let compositor = Arc::new(Compositor::new(&device_arc, surface_format));

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
    let compute_service = Arc::new(ComputeService::new(resources.clone()));

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
    
    // Флаг для graceful shutdown
    let running = Arc::new(AtomicBool::new(true));

    let sys_clone = system_service.clone();
    plugin_module::load_services(
        dispatcher.clone(), 
        resources.clone(), 
        &config_dir, 
        move |id, cfg| {
            sys_clone.register_config(id, cfg);
        },
        &mut plugin_windows,
    ).await?;

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    let running_polling = running.clone();
    let frame_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let frame_pending_clone = frame_pending.clone();
    let last_int_clone = last_interaction_time.clone();
    let last_rend_clone = last_render_time.clone();
    let frame_wake_clone = frame_wake.clone();
    let event_queue_clone = event_queue.clone();

    // 1. ЦИКЛ ОБРАБОТКИ ЗАДАЧ (POLLING)
    tokio::spawn(async move {
        while running_polling.load(Ordering::Relaxed) {
            let has_tasks = {
                let tasks = d_clone.tasks.lock().unwrap();
                !tasks.is_empty()
            };

            if has_tasks || is_visible_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = d_clone.poll_all_tasks().await;
                
                let (needs_frame, eq_len, int_elapsed, rend_elapsed) = {
                    let eq = event_queue_clone.lock().unwrap();
                    let last_int = last_int_clone.lock().unwrap();
                    let last_rend = last_rend_clone.lock().unwrap();
                    
                    let eq_len = eq.len();
                    let int_elapsed = last_int.elapsed().as_millis();
                    let rend_elapsed = last_rend.elapsed().as_millis();
                    let needs = !eq.is_empty() || int_elapsed < 500 || rend_elapsed > 1000;
                    (needs, eq_len, int_elapsed, rend_elapsed)
                };
                
                veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, 
                    "[HOST] Polling: needs_frame={}, eq={}, int_elapsed={}ms, rend_elapsed={}ms, frame_pending={}",
                    needs_frame, eq_len, int_elapsed, rend_elapsed,
                    frame_pending_clone.load(std::sync::atomic::Ordering::SeqCst));

                if needs_frame && !frame_pending_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    frame_pending_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST] Polling loop notifying render thread");
                    frame_wake_clone.notify_one();
                }

                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Polling loop exiting...");
    });

    // 2. ЦИКЛ ОТРИСОВКИ (FRAME PACING)
    let mut last_render_finish = std::time::Instant::now();
    let frame_wake_render = frame_wake.clone();
    let window_render = window.clone();
    let frame_pending_render = frame_pending.clone();
    let running_render = running.clone();
    
    let dispatcher_render = dispatcher.clone();
    let event_queue_render = event_queue.clone();
    let actual_fps_render = actual_fps.clone();

    // 2. ЦИКЛ ОТРИСОВКИ (FRAME PACING)
    tokio::spawn(async move {
        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Render loop started");
        while running_render.load(Ordering::Relaxed) {
            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Waiting for frame notification...");
            frame_wake_render.notified().await;
            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Got frame notification");
            
            let start_redraw = std::time::Instant::now();
            let mut events = {
                let mut eq = event_queue_render.lock().unwrap();
                std::mem::take(&mut *eq)
            };

            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Acquiring locks for frame event...");
            let dt = start_redraw.duration_since(last_render_finish).as_secs_f32();
            let fps = *actual_fps_render.lock().unwrap();
            
            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Building frame event...");
            events.push(veldmap_host_core::app::UiEvent {
                event: Some(veldmap_host_core::app::ui_event::Event::Frame(veldmap_host_core::app::FrameEvent {
                    dt,
                    actual_fps: fps,
                    monitor_fps: 60,
                    surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: 0, size: 0, content_hash: Vec::new() }),
                })),
                ..Default::default()
            });

            for (idx, ev) in events.iter().enumerate() {
                let payload = ev.encode_to_vec();
                veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Calling data-browser::handle_ui_event event #{}", idx);
                let result = dispatcher_render.call("data-browser", "handle_ui_event", payload, 0).await;
                match &result {
                    Ok(_) => veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] data-browser::handle_ui_event returned OK"),
                    Err(e) => veldmap_host_core::verror!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] data-browser::handle_ui_event FAILED: {}", e),
                }
            }

            window_render.request_redraw();
            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Resetting frame_pending");
            frame_pending_render.store(false, std::sync::atomic::Ordering::SeqCst);
            veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-RENDER] Frame done, last_render_finish updated");
            last_render_finish = std::time::Instant::now();
        }
        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Render loop exiting...");
    });

    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Core ready. Render loop started...");
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Veldmap Iroh Node listening. Node ID: {}", endpoint.id());

    // Начальная инициализация: проверяем актуальные размеры окна
    let actual_size = window.inner_size();
    
    // Если размеры изменились - переконфигурируем surface
    if actual_size.width != config.width || actual_size.height != config.height {
        config.width = actual_size.width.max(1);
        config.height = actual_size.height.max(1);
        surface.configure(&device_arc, &config);
    }

    // Начальная инициализация: отправляем Resize событие для инициализации UI
    let initial_ev = veldmap_host_core::app::UiEvent {
        event: Some(veldmap_host_core::app::ui_event::Event::Resize(
            veldmap_host_core::app::ResizeEvent {
                width: config.width,
                height: config.height,
                scale_factor: window.scale_factor() as f32,
                surface_handle: Some(veldmap_host_core::core::ResourceHandle {
                    id: 0,
                    size: 0,
                    content_hash: Vec::new(),
                }),
            }
        )),
        ..Default::default()
    };
    event_queue.lock().unwrap().push(initial_ev);
    
    // Принудительная первая отрисовка
    window.request_redraw();

    // Счётчик переконфигураций surface (для отладки)
    let surface_config_count = AtomicUsize::new(0);

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
                            // SURFACE_ID (0) означает, что UI сервис ещё не готов
                            // Не очищаем bind_group, просто запрашиваем redraw
                        } else if Some(id) != app_texture_id {
                            if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(id, 0) {
                                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let bind_group = compositor.create_bind_group(&device_arc, &view);
                                app_texture_id = Some(id);
                                app_bind_group = Some(bind_group);
                            } else {
                                veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Failed to get texture resource for id {}", id);
                            }
                        }
                        window.request_redraw();
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Close requested, shutting down...");
                running.store(false, Ordering::Relaxed);
                frame_wake.notify_one(); // Wake up render loop to exit
                window_target.exit();
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                let new_width = size.width.max(1);
                let new_height = size.height.max(1);
                veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Resized event: new={}x{}, current={}x{}", new_width, new_height, config.width, config.height);
                // Переконфигурируем только если размеры реально изменились
                if new_width != config.width || new_height != config.height {
                    let count = surface_config_count.fetch_add(1, Ordering::Relaxed) + 1;
                    config.width = new_width;
                    config.height = new_height;
                    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface reconfigure #{}: {}x{}", count, config.width, config.height);
                    surface.configure(&device_arc, &config);
                } else {
                    veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Skipping reconfigure - same size");
                }
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
                veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_HOST_RENDER, "[HOST-EVENT] RedrawRequested received");
                let start_redraw = std::time::Instant::now();
                let frame = match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Lost) => {
                        veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface lost, reconfiguring...");
                        surface.configure(&device_arc, &config);
                        window.request_redraw();
                        return;
                    }
                    Err(wgpu::SurfaceError::Outdated) => {
                        veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface outdated, requesting redraw...");
                        window.request_redraw();
                        return;
                    }
                    Err(e) => { 
                        veldmap_host_core::verror!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface error: {}", e); 
                        return; 
                    }
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device_arc.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                // Execute pending plugin render ops (queued by compute service)
                {
                    let mut ops = veldmap_host_compute::PENDING_OPS.lock().unwrap();
                    for op in ops.drain(..) {
                        if let Some(veldmap_host_core::resources::Resource::TextureView(target_view)) = 
                            resources.get_resource(op.target_view_id, op.instance_id) 
                        {
                            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("Plugin Render Pass"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &target_view,
                                    resolve_target: None,
                                    ops: wgpu::Operations { 
                                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), 
                                        store: wgpu::StoreOp::Store 
                                    },
                                    depth_slice: None,
                                })],
                                depth_stencil_attachment: None,
                                multiview_mask: None,
                                timestamp_writes: None,
                                occlusion_query_set: None,
                            });

                            let _ = veldmap_host_compute::execute_render_commands(
                                &mut rp, &op.command_buffer, &resources, 2048, 2048, op.instance_id
                            );
                        }
                    }
                }

                // Compose final frame: clear + blit UI
                {
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Compositor Pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations { 
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }), 
                                store: wgpu::StoreOp::Store 
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });

                    if let Some(bg) = &app_bind_group {
                        compositor.blit_ui(&mut rp, bg);
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
