#![recursion_limit = "512"]

mod compositor;
use compositor::Compositor;

use crate::app_service::AppCommand;
mod app_service;

mod window;

use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use prost::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- 0. ПАРСИНГ ПУТЕЙ И ЧТЕНИЕ КОНФИГА ЛОГОВ ---
    let args: Vec<String> = std::env::args().collect();
    let config_dir = args.iter().position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "config".to_string());

    // --- 1. ИНИЦИАЛИЗАЦИЯ КОНФИГУРАЦИЙ И ЛОГИРОВАНИЯ ---
    let host_config = Arc::new(veldmap_host_core::config::load_host_config(&config_dir)?);
    veldmap_host_core::setup::init_logging(&config_dir, &host_config)?;

    // --- 2. СКАНИРОВАНИЕ КОНФИГОВ ПЛАГИНОВ ---
    let plugin_windows = window::extract_window_configs(&host_config);

    if plugin_windows.has_windows() {
        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Found plugin window configs, selecting first one");
    }

    let (window_width, window_height, window_title, _ui_scale, window_resizable, window_fullscreen) = plugin_windows
        .first()
        .map(|(name, _)| {
            let cfg = plugin_windows.get(name).expect("window config exists for plugin returned by first()");
            veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Using window config from plugin '{}'", name);
            if let Some(pos) = &cfg.position {
                veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Requested window position: ({}, {})", pos.x, pos.y);
            }
            (cfg.width as f64, cfg.height as f64, cfg.title.clone(), cfg.ui_scale, cfg.resizable, cfg.fullscreen)
        })
        .unwrap_or_else(|| {
            veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "No plugin window config found, using defaults");
            (1024.0, 768.0, "VeldMap".to_string(), 1.0f32, true, false)
        });

    let event_loop = EventLoopBuilder::<AppCommand>::with_user_event().build()?;
    let window = Arc::new(WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(winit::dpi::LogicalSize::new(window_width, window_height))
        .with_resizable(window_resizable)
        .with_fullscreen(window_fullscreen.then_some(winit::window::Fullscreen::Borderless(None)))
        .build(&event_loop)?);

    // --- 3. ИНИЦИАЛИЗАЦИЯ WGPU ---
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Creating wgpu instance (Vulkan only)...");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::all(),
        ..Default::default()
    });
    
    veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Creating surface...");
    let surface = instance.create_surface(window.clone())?;
    
    let (_adapter, device_arc, queue_arc, mut config, surface_format) = 
        veldmap_host_core::setup::init_wgpu(&instance, &surface, window.inner_size().width, window.inner_size().height).await?;

    // --- 4. ИНИЦИАЛИЗАЦИЯ ЯДРА И СЕРВИСОВ ---
    let core_services = veldmap_host_core::setup::init_core_services(device_arc.clone(), queue_arc.clone(), surface_format, host_config).await?;
    let ctx = core_services;
    let memory = ctx.memory.clone();
    let dispatcher = ctx.dispatcher.clone();
    let graphics = ctx.graphics.clone();

    // Initialize compositor for final UI composition
    let compositor = Arc::new(Compositor::new(&device_arc, surface_format));

    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut app_texture_id: Option<u64> = None;
    let mut cursor_pos = (0.0f32, 0.0f32);

    let actual_fps = Arc::new(std::sync::Mutex::new(60.0f32));
    let last_frame_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

    veldmap_host_system::register_services(ctx.clone());

    // Register Modular Services
    #[cfg(target_os = "linux")]
    veldmap_host_fs::register_services(ctx.clone());
    
    veldmap_host_network::register_services(ctx.clone());
    
    let proxy = event_loop.create_proxy();
    app_service::register_services(ctx.clone(), proxy);

    veldmap_host_core::plugins::load_services(
        ctx.clone(),
    ).await?;

    // Отправляем фейковое событие изменения окна, чтобы инициализировать UI
    let initial_ev = veldmap_host_core::app::UiEvent {
        event: Some(veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent {
            width: config.width,
            height: config.height,
            scale_factor: 1.0,
            surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: 0, size: 0, content_hash: Vec::new() }),
        })),
        ..Default::default()
    };
    
    dispatcher.publish("ui-service/handle_ui_event", initial_ev.encode_to_vec());

    // Все сервисы загружены и размер окна разослан: UI-модули могут слать первый view.
    dispatcher.publish("app/ready", Vec::new());

    // Принудительная первая отрисовка
    window.request_redraw();

    // Счётчик переконфигураций surface (для отладки)
    let surface_config_count = AtomicUsize::new(0);

    event_loop.run(move |event, window_target| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(cmd) => {
                let AppCommand::Draw(id) = cmd;
                if id != veldmap_host_core::SURFACE_ID && Some(id) != app_texture_id {
                    if let Some(texture) = memory.get_texture(id).map(|(t, _, _, _)| t) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let bind_group = compositor.create_bind_group(&device_arc, &view);
                        veldmap_host_core::vinfo!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Compositor bound app texture {}", id);
                        app_texture_id = Some(id);
                        app_bind_group = Some(bind_group);
                    } else {
                        veldmap_host_core::vwarn!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Draw command for unknown texture {}", id);
                    }
                }
                window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                window_target.exit();
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                let new_width = size.width.max(1);
                let new_height = size.height.max(1);
                if new_width != config.width || new_height != config.height {
                    config.width = new_width;
                    config.height = new_height;
                    surface.configure(&device_arc, &config);
                    let count = surface_config_count.fetch_add(1, Ordering::Relaxed) + 1;
                    veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface reconfigured (count: {})", count);
                }
                window.request_redraw();
                let ev = veldmap_host_core::app::UiEvent {
                    event: Some(veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent {
                        width: new_width,
                        height: new_height,
                        scale_factor: 1.0,
                        surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: 0, size: 0, content_hash: Vec::new() }),
                    })),
                    ..Default::default()
                };
                dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let now = std::time::Instant::now();
                let dt = now.duration_since(*last_frame_time.lock().unwrap()).as_secs_f32();
                *last_frame_time.lock().unwrap() = now;
                let fps = *actual_fps.lock().unwrap();

                let ev = veldmap_host_core::app::UiEvent {
                    event: Some(veldmap_host_core::app::ui_event::Event::Frame(veldmap_host_core::app::FrameEvent { 
                        dt, 
                        actual_fps: fps,
                        monitor_fps: 60,
                        surface_handle: Some(veldmap_host_core::core::ResourceHandle { id: 0, size: 0, content_hash: Vec::new() }),
                    })),
                    ..Default::default()
                };
                dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());

                let frame = match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Lost) => {
                        surface.configure(&device_arc, &config);
                        let count = surface_config_count.fetch_add(1, Ordering::Relaxed) + 1;
                        veldmap_host_core::vdebug!(veldmap_host_core::logging::FLAG_HOST_RENDER, "Surface reconfigured after loss (count: {})", count);
                        window.request_redraw();
                        return;
                    }
                    Err(_) => return,
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device_arc.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                {
                    let mut ops = veldmap_host_core::PENDING_OPS.lock().unwrap();
                    for op in ops.drain(..) {
                        if let Some(veldmap_host_core::graphics::Resource::GpuObj(
                            veldmap_host_core::graphics::GpuObject::TextureView(target_view)
                        )) = graphics.get_resource(op.target_view_id, op.instance_id)
                            {
                                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("Plugin Render Pass"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: &target_view,
                                        resolve_target: None,
                                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                                        depth_slice: None,
                                    })],
                                    depth_stencil_attachment: None,
                                    ..Default::default()
                                });
                            let _ = veldmap_host_core::graphics::execute_render_commands(&mut rp, &op.command_buffer, &graphics, 2048, 2048, op.instance_id);
                        }
                    }
                }

                {
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Compositor Pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }), store: wgpu::StoreOp::Store },
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

                // Keep the frame loop alive: plugins render in response to Frame
                // events published above, so redraws must not depend on plugins
                // sending Draw commands first (paced by present_mode vsync).
                window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                let ev = veldmap_host_core::app::UiEvent { 
                    event: Some(veldmap_host_core::app::ui_event::Event::CursorMoved(veldmap_host_core::app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })),
                    ..Default::default()
                };
                dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state: button_state, button, .. }, .. } => {
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
                dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                let ev = veldmap_host_core::app::UiEvent { 
                    event: Some(veldmap_host_core::app::ui_event::Event::Scroll(veldmap_host_core::app::ScrollEvent { 
                        delta_x: match delta { winit::event::MouseScrollDelta::LineDelta(x, _) => x * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.x as f32 },
                        delta_y: match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 }
                    })),
                    ..Default::default()
                };
                dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());
            }
            Event::WindowEvent { event: WindowEvent::KeyboardInput { event: input, .. }, .. } => {
                if let winit::keyboard::PhysicalKey::Code(kc) = input.physical_key {
                    let ev = veldmap_host_core::app::UiEvent { 
                        event: Some(veldmap_host_core::app::ui_event::Event::Key(veldmap_host_core::app::KeyEvent { 
                            key_code: kc as u32,
                            pressed: input.state == winit::event::ElementState::Pressed
                        })),
                        ..Default::default()
                    };
                    dispatcher.publish("ui-service/handle_ui_event", ev.encode_to_vec());
                }
            }
            _ => (),
        }
    })?;
    Ok(())
}
