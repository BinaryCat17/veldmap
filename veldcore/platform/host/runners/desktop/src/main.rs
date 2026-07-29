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

use veldmap_host_core::app;
use veldmap_host_bindings::app as app_bus;

/// Окно, созданное по декларации модуля-владельца.
/// Хост не знает, кто рендерит: владелец аллоцирует текстуру сам, делегирует её
/// рендереру write-lease'ом и аттачит хосту через app/set_surface.
struct HostWindow {
    owner: String,
    size: (u32, u32),
    /// Композитируемая поверхность: (texture_id, bind group для блита).
    /// None до первого set_surface — окно рисует фоновый цвет.
    surface: Option<(u64, wgpu::BindGroup)>,
}

/// Публикация UI-события в нейтральный топик app/on_ui_event.
/// Адресация — в данных: plugin_id называет владельца окна.
fn publish_ui_event(p: &impl veldmap_host_bindings::Publisher, owner: &str, event: app::ui_event::Event) {
    let ev = app::UiEvent {
        plugin_id: owner.to_string(),
        event: Some(event),
    };
    app_bus::emit::on_ui_event(p, &ev);
}

/// Битовая маска модификаторов для KeyEvent: 1=Shift, 2=Ctrl, 4=Alt, 8=Super.
fn modifiers_bits(m: winit::keyboard::ModifiersState) -> u32 {
    let mut bits = 0;
    if m.shift_key() { bits |= 1; }
    if m.control_key() { bits |= 2; }
    if m.alt_key() { bits |= 4; }
    if m.super_key() { bits |= 8; }
    bits
}

/// Сообщает владельцу окна размер и формат требуемой поверхности.
fn publish_window_resized(p: &impl veldmap_host_bindings::Publisher, owner: &str, width: u32, height: u32, scale_factor: f32, format: i32) {
    let ev = app::WindowResized {
        plugin_id: owner.to_string(),
        width,
        height,
        scale_factor,
        format,
    };
    app_bus::emit::on_window_resized(p, &ev, owner);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── 0. Конфигурация и логирование ─────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let config_dir = args.iter().position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "config".to_string());

    // Секреты для ${VAR} в конфигах — из .env, до всякого чтения конфигов.
    // Кандидаты: cwd (так запускает лаунчер) и корень проекта относительно
    // каталога конфигов (прямой запуск бинарника из любого места).
    for candidate in [std::path::PathBuf::from(".env"),
                      std::path::Path::new(&config_dir).join("../../.env")] {
        if candidate.exists() {
            veldmap_host_core::config::load_dotenv(&candidate);
            break;
        }
    }

    let host_config = Arc::new(veldmap_host_core::config::load_host_config(&config_dir)?);
    veldmap_host_core::setup::init_logging(&config_dir, &host_config)?;

    // ── 1. Окна: декларации владельцев ─────────────────────────────────────
    let declared = window::extract_window_configs(&host_config);
    let (owner_name, win_cfg) = match declared.len() {
        1 => declared.into_iter().next().expect("len checked"),
        0 => anyhow::bail!("No module declares a window; the desktop runner has nothing to present"),
        n => anyhow::bail!("{} modules declare windows, but the desktop runner supports exactly one for now", n),
    };
    log::info!(target: "render", "Window '{}': owner '{}'", win_cfg.title, owner_name);

    let event_loop = EventLoop::new()?;
    let mut builder = WindowBuilder::new()
        .with_title(win_cfg.title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(win_cfg.width as f64, win_cfg.height as f64))
        .with_resizable(win_cfg.resizable)
        .with_fullscreen(win_cfg.fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
    if let Some(pos) = &win_cfg.position {
        builder = builder.with_position(winit::dpi::LogicalPosition::new(pos.x as f64, pos.y as f64));
    }
    let winit_window = Arc::new(builder.build(&event_loop)?);

    // ── 2. GPU ──────────────────────────────────────────────────────────────
    log::info!(target: "render", "Creating wgpu instance (Vulkan only)...");
    // Валидация Vulkan — только по запросу через env (WGPU_VALIDATION=1 и т.п.):
    // InstanceFlags::all() в релизе включал полный validation layer и тормозил.
    // ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER обязателен: Dozen (Vulkan поверх
    // DX12 в WSL) — non-conformant драйвер, без флага остаётся только llvmpipe.
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: (wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER).with_env(),
        ..Default::default()
    });
    let surface = instance.create_surface(winit_window.clone())?;
    let (_adapter, device_arc, queue_arc, mut config, surface_format) =
        veldmap_host_core::setup::init_wgpu(&instance, &surface, winit_window.inner_size().width, winit_window.inner_size().height).await?;

    // ── 3. Ядро и сервисы ──────────────────────────────────────────────────
    let ctx = veldmap_host_core::setup::init_core_services(device_arc.clone(), queue_arc.clone(), surface_format, host_config).await?;
    let dispatcher = ctx.dispatcher.clone();
    let graphics = ctx.graphics.clone();

    // Нативные модули этого раннера — по списку из runner.yaml (крейт
    // композиции генерирует buildgen). Приём поверхностей окон входит сюда
    // как модуль app.
    veldmap_desktop_modules::register_all(ctx.clone());

    // Контракт app реализует сам раннер (оконная система): его события
    // штампуем именем app, как у остальных сервисов. app-модуль уже
    // зарегистрирован в register_all выше; без него — fallback на хост (0),
    // что для событий окна семантически честно.
    let app_pub = dispatcher.publisher_of("app");

    veldmap_host_core::plugins::load_services(ctx.clone()).await?;
    if dispatcher.instance_of(&owner_name).is_none() {
        anyhow::bail!("Window owner '{}' is not a loaded service", owner_name);
    }

    let compositor = Compositor::new(&device_arc, surface_format);
    let size = winit_window.inner_size();
    let mut hw = HostWindow {
        owner: owner_name,
        size: (size.width, size.height),
        surface: None,
    };

    // ── 4. Детерминированный bootstrap: размер → готовность ────────────────
    // Владелец в ответ аллоцирует текстуру, делегирует её рендереру и аттачит
    // сюда через app/set_surface; до этого окно рисует фоновый цвет.
    let format_proto = graphics.get_surface_format_proto();
    // Эффективный масштаб: реальный DPI из winit, но не ниже win_cfg.ui_scale.
    // На X11/WSLg winit часто репортит 1.0 даже на HiDPI (Xft.dpi не задан),
    // поэтому конфиг задаёт нижнюю границу.
    let effective_scale = move |w: &winit::window::Window| (w.scale_factor() as f32).max(win_cfg.ui_scale);
    publish_window_resized(&app_pub, &hw.owner, hw.size.0, hw.size.1, effective_scale(&winit_window), format_proto);
    app_bus::emit::on_ready(&app_pub);

    winit_window.request_redraw();

    // ── 5. Кадровый цикл ────────────────────────────────────────────────────
    let mut cursor_pos = (0.0f32, 0.0f32);
    // CursorMoved коалесцируется до одного события на кадр: каждый move — это
    // отдельный вызов wasm-актера ui-service, и поток движений мыши (40-125/с)
    // раньше создавал бэклог очереди с секундной задержкой кликов.
    let mut cursor_dirty = false;
    let mut last_frame_time = std::time::Instant::now();
    // Состояние модификаторов: winit шлёт его отдельно от KeyboardInput,
    // поэтому трекаем здесь и прикладываем к каждому KeyEvent.
    let mut key_modifiers = winit::keyboard::ModifiersState::empty();

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
                    hw.size = (width, height);

                    // Старая поверхность блитится растянутой, пока владелец
                    // не приаттачит новую нужного размера.
                    publish_window_resized(&app_pub, &hw.owner, width, height, effective_scale(&winit_window), format_proto);
                }
                winit_window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let now = std::time::Instant::now();
                let dt = now.duration_since(last_frame_time).as_secs_f32();
                last_frame_time = now;

                // Коалесцированный курсор — не чаще одного события на кадр.
                if cursor_dirty {
                    cursor_dirty = false;
                    publish_ui_event(&app_pub, &hw.owner, app::ui_event::Event::CursorMoved(
                        app::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 },
                    ));
                }

                publish_ui_event(&app_pub, &hw.owner, app::ui_event::Event::Frame(app::FrameEvent {
                    dt,
                    actual_fps: if dt > 0.0 { 1.0 / dt } else { 0.0 },
                    monitor_fps: 60,
                }));

                // Атомарный свап поверхности, если владелец приаттачил новую.
                // Права проверены на входе (модуль app + фасад Surfaces);
                // здесь остаётся только подмена между кадрами.
                if let Some(texture_id) = ctx.surfaces.take(&hw.owner) {
                    match ctx.memory.get_texture(texture_id) {
                        Some((texture, _, _, _)) => {
                            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let bind_group = compositor.create_bind_group(&device_arc, &view);
                            hw.surface = Some((texture_id, bind_group));
                            log::debug!(target: "render", "Window '{}' surface attached: texture {}", hw.owner, texture_id);
                        }
                        None => log::warn!(target: "render", "set_surface for '{}' names unknown texture {}", hw.owner, texture_id),
                    }
                }

                let frame = match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Lost) => {
                        surface.configure(&device_arc, &config);
                        log::debug!(target: "render", "Surface reconfigured after loss");
                        winit_window.request_redraw();
                        return;
                    }
                    Err(_) => return,
                };
                let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device_arc.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                // 1) Render-опы модулей — в их таргеты (обычно текстура окна).
                for op in graphics.take_pending_ops() {
                    let target = graphics.get_gpu(op.target_view_id, op.instance_id);
                    if let Ok(veldmap_host_core::registry::GpuObject::TextureView(target_view)) = target {
                        // Размеры целевой текстуры (не окна) — viewport/scissor
                        // клампятся по таргету. У каждого view размеры записаны
                        // при создании; None — view чужой или уже освобождён.
                        let Some((target_w, target_h)) = graphics.get_texture_view_size(op.target_view_id, op.instance_id) else {
                            log::error!(target: "render", "Render op targets view {} with unknown size, skipped", op.target_view_id);
                            continue;
                        };
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
                            &mut rp, &op.command_buffer, &graphics, target_w, target_h, op.instance_id,
                        );
                    } else {
                        log::warn!(target: "render", "Render op targets unknown view {}", op.target_view_id);
                    }
                }

                // 2) Блит приаттаченной поверхности в свопчейн.
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
                    if let Some((_, bind_group)) = &hw.surface {
                        compositor.blit_ui(&mut rp, bind_group);
                    }
                }

                queue_arc.lock().unwrap().submit(Some(encoder.finish()));
                frame.present();

                // Модули рендерят в ответ на Frame-события: цикл кадров живёт
                // на хосте (темп задаёт present_mode/vsync).
                winit_window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                cursor_dirty = true;
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state: button_state, button, .. }, .. } => {
                let b_idx = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                publish_ui_event(&app_pub, &hw.owner, app::ui_event::Event::Click(
                    app::ClickEvent {
                        button: b_idx,
                        pressed: button_state == winit::event::ElementState::Pressed,
                        x: cursor_pos.0,
                        y: cursor_pos.1,
                    },
                ));
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                publish_ui_event(&app_pub, &hw.owner, app::ui_event::Event::Scroll(
                    app::ScrollEvent {
                        delta_x: match delta { winit::event::MouseScrollDelta::LineDelta(x, _) => x * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.x as f32 },
                        delta_y: match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 },
                    },
                ));
            }
            Event::WindowEvent { event: WindowEvent::ModifiersChanged(m), .. } => {
                key_modifiers = m.state();
            }
            Event::WindowEvent { event: WindowEvent::KeyboardInput { event: input, .. }, .. } => {
                if let winit::keyboard::PhysicalKey::Code(kc) = input.physical_key {
                    let pressed = input.state == winit::event::ElementState::Pressed;
                    publish_ui_event(&app_pub, &hw.owner, app::ui_event::Event::Key(
                        app::KeyEvent {
                            key_code: kc as u32,
                            pressed,
                            // Текст есть только у нажатий печатных клавиш.
                            text: if pressed { input.text.as_ref().map(|t| t.to_string()).unwrap_or_default() } else { String::new() },
                            modifiers: modifiers_bits(key_modifiers),
                        },
                    ));
                }
            }
            _ => (),
        }
    })?;
    Ok(())
}
