#![recursion_limit = "512"]

mod compositor;
use compositor::Compositor;

mod window;

use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use std::sync::Arc;
use prost::Message;

use veldmap_host_core::logging::FLAG_HOST_RENDER;
use veldmap_host_core::{vinfo, vwarn, vdebug};

/// Well-known имя интерфейса UI-рендерера (как `fs`, `network`, `system`).
/// Реализация подменяется в services.json — любой модуль под этим именем,
/// реализующий set_view/handle_ui_event, исполняет окна десктоп-раннера.
const UI_SERVICE: &str = "ui-service";

/// Окно, созданное по декларации модуля, и его render-target.
/// Владелец текстуры — задекларировавший модуль; писатель — его рендерер.
struct HostWindow {
    owner: String,
    renderer: String,
    texture_id: u64,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// Публикация UI-события в input-метод рендерера окна.
fn publish_ui_event(
    dispatcher: &veldmap_host_core::dispatcher::Dispatcher,
    hw: &HostWindow,
    event: veldmap_host_core::app::ui_event::Event,
) {
    let ev = veldmap_host_core::app::UiEvent {
        plugin_id: hw.owner.clone(),
        event: Some(event),
        ..Default::default()
    };
    dispatcher.publish(&format!("{}/handle_ui_event", hw.renderer), ev.encode_to_vec());
}

fn target_handle(texture_id: u64) -> Option<veldmap_host_core::core::ResourceHandle> {
    Some(veldmap_host_core::core::ResourceHandle { id: texture_id, size: 0, content_hash: Vec::new() })
}

/// Создаёт render-target текстуру окна: владелец — модуль окна,
/// write-lease — рендереру. Возвращает id текстуры и bind group для блита.
fn create_window_target(
    ctx: &veldmap_host_core::setup::HostContext,
    compositor: &Compositor,
    device: &wgpu::Device,
    width: u32,
    height: u32,
    owner_instance: u32,
    renderer_instance: u32,
) -> (u64, wgpu::BindGroup) {
    let format_proto = ctx.graphics.get_surface_format_proto();
    // COPY_DST | TEXTURE_BINDING | RENDER_ATTACHMENT
    let texture_id = ctx.memory.alloc_texture(width, height, format_proto, 2 | 4 | 16, false, owner_instance);
    ctx.registry.update_lease(texture_id, |lease| lease.add_writer(renderer_instance));

    let (texture, _, _, _) = ctx.memory.get_texture(texture_id).expect("freshly created window texture exists");
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = compositor.create_bind_group(device, &view);
    (texture_id, bind_group)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── 0. Конфигурация и логирование ─────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let config_dir = args.iter().position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "config".to_string());

    let host_config = Arc::new(veldmap_host_core::config::load_host_config(&config_dir)?);
    veldmap_host_core::setup::init_logging(&config_dir, &host_config)?;

    // ── 1. Окна: декларации владельцев ─────────────────────────────────────
    let declared = window::extract_window_configs(&host_config);
    let (owner_name, win_cfg) = match declared.len() {
        1 => declared.into_iter().next().expect("len checked"),
        0 => anyhow::bail!("No module declares a window; the desktop runner has nothing to present"),
        n => anyhow::bail!("{} modules declare windows, but the desktop runner supports exactly one for now", n),
    };
    let renderer_name = UI_SERVICE.to_string();
    vinfo!(FLAG_HOST_RENDER, "Window '{}': owner '{}', renderer '{}'", win_cfg.title, owner_name, renderer_name);

    let event_loop = EventLoop::new()?;
    let winit_window = Arc::new(WindowBuilder::new()
        .with_title(win_cfg.title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(win_cfg.width as f64, win_cfg.height as f64))
        .with_resizable(win_cfg.resizable)
        .with_fullscreen(win_cfg.fullscreen.then_some(winit::window::Fullscreen::Borderless(None)))
        .build(&event_loop)?);

    // ── 2. GPU ──────────────────────────────────────────────────────────────
    vinfo!(FLAG_HOST_RENDER, "Creating wgpu instance (Vulkan only)...");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::all(),
        ..Default::default()
    });
    let surface = instance.create_surface(winit_window.clone())?;
    let (_adapter, device_arc, queue_arc, mut config, surface_format) =
        veldmap_host_core::setup::init_wgpu(&instance, &surface, winit_window.inner_size().width, winit_window.inner_size().height).await?;

    // ── 3. Ядро и сервисы ──────────────────────────────────────────────────
    let ctx = veldmap_host_core::setup::init_core_services(device_arc.clone(), queue_arc.clone(), surface_format, host_config).await?;
    let dispatcher = ctx.dispatcher.clone();
    let graphics = ctx.graphics.clone();

    veldmap_host_system::register_services(ctx.clone());
    #[cfg(target_os = "linux")]
    veldmap_host_fs::register_services(ctx.clone());
    veldmap_host_network::register_services(ctx.clone());

    let instance_ids = veldmap_host_core::plugins::load_services(ctx.clone()).await?;

    // ── 4. Render-target окна ──────────────────────────────────────────────
    let owner_instance = *instance_ids.get(&owner_name)
        .ok_or_else(|| anyhow::anyhow!("Window owner '{}' is not a loaded service", owner_name))?;
    let renderer_instance = *instance_ids.get(&renderer_name)
        .ok_or_else(|| anyhow::anyhow!(
            "No '{}' service loaded: the desktop runner needs a UI renderer to present windows (add one to services.json)",
            renderer_name,
        ))?;

    let compositor = Compositor::new(&device_arc, surface_format);
    let size = winit_window.inner_size();
    let (texture_id, bind_group) = create_window_target(
        &ctx, &compositor, &device_arc, size.width, size.height, owner_instance, renderer_instance,
    );
    let mut hw = HostWindow {
        owner: owner_name,
        renderer: renderer_name,
        texture_id,
        bind_group,
        size: (size.width, size.height),
    };
    vinfo!(FLAG_HOST_RENDER, "Window target texture {} ({}x{})", hw.texture_id, hw.size.0, hw.size.1);

    // ── 5. Детерминированный bootstrap: размер → готовность ────────────────
    publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent {
        width: hw.size.0,
        height: hw.size.1,
        scale_factor: 1.0,
        surface_handle: target_handle(hw.texture_id),
    }));
    dispatcher.publish("app/ready", Vec::new());

    winit_window.request_redraw();

    // ── 6. Кадровый цикл ────────────────────────────────────────────────────
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_frame_time = std::time::Instant::now();

    event_loop.run(move |event, window_target| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                window_target.exit();
            }
            Event::WindowEvent { event: WindowEvent::Resized(new_size), .. } => {
                let width = new_size.width.max(1);
                let height = new_size.height.max(1);
                if (width, height) != hw.size {
                    config.width = width;
                    config.height = height;
                    surface.configure(&device_arc, &config);

                    // Пересоздаём render-target под новый размер; владение и lease
                    // восстанавливаются, старая текстура освобождается.
                    let old = hw.texture_id;
                    let (tex, bg) = create_window_target(&ctx, &compositor, &device_arc, width, height, owner_instance, renderer_instance);
                    hw.texture_id = tex;
                    hw.bind_group = bg;
                    hw.size = (width, height);
                    ctx.memory.free(old);
                    vdebug!(FLAG_HOST_RENDER, "Window target recreated: {} ({}x{})", hw.texture_id, width, height);

                    publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Resize(veldmap_host_core::app::ResizeEvent {
                        width,
                        height,
                        scale_factor: 1.0,
                        surface_handle: target_handle(hw.texture_id),
                    }));
                }
                winit_window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let now = std::time::Instant::now();
                let dt = now.duration_since(last_frame_time).as_secs_f32();
                last_frame_time = now;

                publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Frame(veldmap_host_core::app::FrameEvent {
                    dt,
                    actual_fps: if dt > 0.0 { 1.0 / dt } else { 0.0 },
                    monitor_fps: 60,
                    surface_handle: target_handle(hw.texture_id),
                }));

                let frame = match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Lost) => {
                        surface.configure(&device_arc, &config);
                        vdebug!(FLAG_HOST_RENDER, "Surface reconfigured after loss");
                        winit_window.request_redraw();
                        return;
                    }
                    Err(_) => return,
                };
                let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device_arc.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                // 1) Render-опы модулей — в их таргеты (обычно текстура окна).
                for op in graphics.take_pending_ops() {
                    let target = graphics.get_resource(op.target_view_id, op.instance_id);
                    if let Some(veldmap_host_core::graphics::Resource::GpuObj(
                        veldmap_host_core::graphics::GpuObject::TextureView(target_view)
                    )) = target {
                        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Module Render Pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &target_view,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                                depth_slice: None,
                            })],
                            depth_stencil_attachment: None,
                            ..Default::default()
                        });
                        let _ = veldmap_host_core::graphics::execute_render_commands(
                            &mut rp, &op.command_buffer, &graphics, hw.size.0, hw.size.1, op.instance_id,
                        );
                    } else {
                        vwarn!(FLAG_HOST_RENDER, "Render op targets unknown view {}", op.target_view_id);
                    }
                }

                // 2) Блит текстуры окна в свопчейн.
                {
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Compositor Pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &surface_view,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }), store: wgpu::StoreOp::Store },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    compositor.blit_ui(&mut rp, &hw.bind_group);
                }

                queue_arc.lock().unwrap().submit(Some(encoder.finish()));
                frame.present();

                // Модули рендерят в ответ на Frame-события: цикл кадров живёт
                // на хосте (темп задаёт present_mode/vsync).
                winit_window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::CursorMoved(
                    veldmap_host_core::app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 },
                ));
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state: button_state, button, .. }, .. } => {
                let b_idx = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Click(
                    veldmap_host_core::app::ClickEvent {
                        button: b_idx,
                        pressed: button_state == winit::event::ElementState::Pressed,
                        x: cursor_pos.0,
                        y: cursor_pos.1,
                    },
                ));
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Scroll(
                    veldmap_host_core::app::ScrollEvent {
                        delta_x: match delta { winit::event::MouseScrollDelta::LineDelta(x, _) => x * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.x as f32 },
                        delta_y: match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 },
                    },
                ));
            }
            Event::WindowEvent { event: WindowEvent::KeyboardInput { event: input, .. }, .. } => {
                if let winit::keyboard::PhysicalKey::Code(kc) = input.physical_key {
                    publish_ui_event(&dispatcher, &hw, veldmap_host_core::app::ui_event::Event::Key(
                        veldmap_host_core::app::KeyEvent {
                            key_code: kc as u32,
                            pressed: input.state == winit::event::ElementState::Pressed,
                        },
                    ));
                }
            }
            _ => (),
        }
    })?;
    Ok(())
}
