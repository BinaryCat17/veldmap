#![recursion_limit = "512"]

mod capture;
mod compositor;
use compositor::Compositor;

mod window;

use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};
use std::sync::{Arc, Mutex};

use veldmap_host_core::app;
use veldmap_host_core::dispatcher::ServicePublisher;
use veldmap_host_core::setup::HostContext;
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

/// Раннер до первого `resumed`: окно ещё не создано, а значит нет ни GPU, ни
/// сервисов — вся инициализация ниже по цепочке начинается с оконного хендла.
struct App<'a> {
    /// Рантайм заводится в `main` руками, а не `#[tokio::main]`: цикл событий
    /// винита синхронный, и асинхронная инициализация внутри `resumed`
    /// иначе оказалась бы `block_on` из уже работающего рантайма.
    runtime: &'a tokio::runtime::Runtime,
    host_config: Arc<veldmap_host_core::config::HostConfig>,
    owner_name: String,
    win_cfg: window::PluginWindowConfig,
    running: Option<Running>,
}

/// Раннер после инициализации: окно, GPU и загруженные сервисы.
struct Running {
    window: Arc<winit::window::Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<Mutex<wgpu::Queue>>,
    compositor: Compositor,
    ctx: Arc<HostContext>,
    app_pub: ServicePublisher,
    hw: HostWindow,
    format_proto: i32,
    /// Нижняя граница масштаба из декларации окна — см. `effective_scale`.
    ui_scale: f32,
    cursor_pos: (f32, f32),
    /// CursorMoved коалесцируется до одного события на кадр: каждый move — это
    /// отдельный вызов wasm-актера ui-service, и поток движений мыши (40-125/с)
    /// иначе копит бэклог очереди с секундной задержкой кликов.
    cursor_dirty: bool,
    last_frame_time: std::time::Instant,
    /// Состояние модификаторов: winit шлёт его отдельно от KeyboardInput,
    /// поэтому трекаем здесь и прикладываем к каждому KeyEvent.
    key_modifiers: winit::keyboard::ModifiersState,
    /// Отладочный прогон по сценарию (VELDMAP_SCRIPT); `None` — обычный запуск.
    script: Option<capture::Script>,
    /// Сценарий дошёл до `exit`. Закрывает окно цикл событий, а не кадр:
    /// `event_loop` виден только обработчику, и завершаться посреди отрисовки
    /// нечестно — кадр надо дорисовать.
    exit_requested: bool,
}

impl Running {
    async fn start(
        event_loop: &ActiveEventLoop,
        host_config: Arc<veldmap_host_core::config::HostConfig>,
        owner_name: String,
        win_cfg: &window::PluginWindowConfig,
    ) -> anyhow::Result<Self> {
        // ── 1. Окно ────────────────────────────────────────────────────────
        let mut attributes = winit::window::Window::default_attributes()
            .with_title(win_cfg.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(win_cfg.width as f64, win_cfg.height as f64))
            .with_resizable(win_cfg.resizable)
            .with_fullscreen(win_cfg.fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
        if let Some(pos) = &win_cfg.position {
            attributes = attributes.with_position(winit::dpi::LogicalPosition::new(pos.x as f64, pos.y as f64));
        }
        let window = Arc::new(event_loop.create_window(attributes)?);

        // ── 2. GPU ──────────────────────────────────────────────────────────
        log::info!(target: "render", "Creating wgpu instance (Vulkan only)...");
        // Валидация Vulkan — только по запросу через env (WGPU_VALIDATION=1 и т.п.):
        // InstanceFlags::all() в релизе включал полный validation layer и тормозил.
        // ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER обязателен: Dozen (Vulkan поверх
        // DX12 в WSL) — non-conformant драйвер, без флага остаётся только llvmpipe.
        // Базой служит `new_without_display_handle`, а не `Default`: у
        // дескриптора его нет, а хендл дисплея нужен только GLES — здесь
        // бэкенд Vulkan, и он его не читает.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            flags: (wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER).with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window.clone())?;
        let size = window.inner_size();
        let (_adapter, device, queue, surface_config, surface_format) =
            veldmap_host_core::setup::init_wgpu(&instance, &surface, size.width, size.height).await?;

        // ── 3. Ядро и сервисы ──────────────────────────────────────────────
        let ctx = veldmap_host_core::setup::init_core_services(
            device.clone(), queue.clone(), surface_format, host_config,
        ).await?;
        let dispatcher = ctx.dispatcher.clone();

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

        let compositor = Compositor::new(&device, surface_format);
        let format_proto = ctx.graphics.get_surface_format_proto();
        // Снимки ложатся туда же, где логи: и то и другое — про последний запуск.
        let script = capture::Script::from_env(
            ctx.config.log_path().parent().unwrap_or(std::path::Path::new(".")),
        );

        Ok(Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            compositor,
            ctx,
            app_pub,
            hw: HostWindow { owner: owner_name, size: (size.width, size.height), surface: None },
            format_proto,
            ui_scale: win_cfg.ui_scale,
            cursor_pos: (0.0, 0.0),
            cursor_dirty: false,
            last_frame_time: std::time::Instant::now(),
            key_modifiers: winit::keyboard::ModifiersState::empty(),
            script,
            exit_requested: false,
        })
    }

    /// Эффективный масштаб: реальный DPI из winit, но не ниже `ui_scale` из
    /// декларации окна. На X11/WSLg winit часто репортит 1.0 даже на HiDPI
    /// (Xft.dpi не задан), поэтому конфиг задаёт нижнюю границу.
    fn effective_scale(&self) -> f32 {
        (self.window.scale_factor() as f32).max(self.ui_scale)
    }

    /// Детерминированный bootstrap: размер → готовность.
    /// Владелец в ответ аллоцирует текстуру, делегирует её рендереру и аттачит
    /// сюда через app/set_surface; до этого окно рисует фоновый цвет.
    fn announce(&self) {
        publish_window_resized(
            &self.app_pub, &self.hw.owner,
            self.hw.size.0, self.hw.size.1, self.effective_scale(), self.format_proto,
        );
        app_bus::emit::on_ready(&self.app_pub);
        self.window.request_redraw();
    }

    fn redraw(&mut self) {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        // Коалесцированный курсор — не чаще одного события на кадр.
        if self.cursor_dirty {
            self.cursor_dirty = false;
            publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::CursorMoved(
                app::CursorMovedEvent { x: self.cursor_pos.0, y: self.cursor_pos.1 },
            ));
        }

        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::Frame(app::FrameEvent {
            dt,
            actual_fps: if dt > 0.0 { 1.0 / dt } else { 0.0 },
            monitor_fps: 60,
        }));

        // Атомарный свап поверхности, если владелец приаттачил новую.
        // Права проверены на входе (модуль app + фасад Surfaces);
        // здесь остаётся только подмена между кадрами.
        if let Some(texture_id) = self.ctx.surfaces.take(&self.hw.owner) {
            match self.ctx.memory.get_texture(texture_id) {
                Some((texture, _, _, _)) => {
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind_group = self.compositor.create_bind_group(&self.device, &view);
                    self.hw.surface = Some((texture_id, bind_group));
                    log::debug!(target: "render", "Window '{}' surface attached: texture {}", self.hw.owner, texture_id);
                }
                None => log::warn!(target: "render", "set_surface for '{}' names unknown texture {}", self.hw.owner, texture_id),
            }
        }

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            // Suboptimal — кадр годный, но свопчейн разошёлся с окном; его
            // рисуем и переконфигурируем со следующего.
            Acquired::Success(f) | Acquired::Suboptimal(f) => f,
            Acquired::Lost | Acquired::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                log::debug!(target: "render", "Surface reconfigured after loss");
                self.window.request_redraw();
                return;
            }
            // Таймаут и перекрытое окно — пропуск кадра без перенастройки:
            // следующий Frame-тик придёт обычным порядком.
            _ => return,
        };
        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // 1) Render-опы модулей — в их таргеты (обычно текстура окна).
        let graphics = self.ctx.graphics.clone();
        for op in graphics.take_pending_ops() {
            let target = graphics.get_gpu(op.target_view_id, op.instance_id);
            // Размеры целевой текстуры (не окна): по ним клампятся
            // viewport и scissor. Записаны в view при создании.
            if let Ok(veldmap_host_core::registry::GpuObject::TextureView {
                view: target_view, width: target_w, height: target_h,
            }) = target {
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
            if let Some((_, bind_group)) = &self.hw.surface {
                self.compositor.blit_ui(&mut rp, bind_group);
            }
        }

        // Показ кадра — операция очереди, а не самой текстуры, поэтому
        // отправка и показ идут под одним захватом: между ними чужой submit
        // в тот же свопчейн недопустим.
        {
            let queue = self.queue.lock().unwrap();
            queue.submit(Some(encoder.finish()));
            queue.present(frame);
        }

        self.play_script();

        // Модули рендерят в ответ на Frame-события: цикл кадров живёт
        // на хосте (темп задаёт present_mode/vsync).
        self.window.request_redraw();
    }

    /// Отыгрывает шаги отладочного сценария, чей срок настал. Идёт после
    /// показа кадра: снимок обязан застать уже отрисованное состояние, а не
    /// то, что было до render-опов этого кадра.
    fn play_script(&mut self) {
        let Some(script) = &mut self.script else { return };
        let due = script.due();
        for action in due {
            match action {
                capture::Action::Move { x, y } => {
                    self.cursor_pos = (x, y);
                    self.cursor_dirty = true;
                }
                capture::Action::Click => {
                    for pressed in [true, false] {
                        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::Click(
                            app::ClickEvent { button: 1, pressed, x: self.cursor_pos.0, y: self.cursor_pos.1 },
                        ));
                    }
                }
                capture::Action::Shot { path } => {
                    let queue = self.queue.lock().unwrap();
                    let frame = capture::FrameSource {
                        device: &self.device,
                        queue: &queue,
                        compositor: &self.compositor,
                        surface: self.hw.surface.as_ref().map(|(_, bind_group)| bind_group),
                        size: self.hw.size,
                        format: self.surface_config.format,
                    };
                    match capture::shoot(frame, &path) {
                        Ok(()) => log::info!(target: "render", "Снимок: {}", path.display()),
                        Err(e) => log::error!(target: "render", "Снимок '{}' не сделан: {:#}", path.display(), e),
                    }
                }
                capture::Action::Exit => self.exit_requested = true,
            }
        }
    }
}

impl ApplicationHandler for App<'_> {
    /// Единственная точка инициализации: winit создаёт окно только изнутри
    /// цикла, а от окна зависит вся остальная цепочка (поверхность → устройство
    /// → сервисы). Повторный `resumed` — законное событие, не повод
    /// переинициализироваться.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        let started = self.runtime.block_on(Running::start(
            event_loop,
            self.host_config.clone(),
            self.owner_name.clone(),
            &self.win_cfg,
        ));
        match started {
            Ok(running) => {
                running.announce();
                self.running = Some(running);
            }
            Err(e) => {
                log::error!(target: "render", "Runner failed to start: {:#}", e);
                event_loop.exit();
            }
        }
    }

    /// Сценарий дошёл до `exit` — закрываемся здесь, между кадрами.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.as_ref().is_some_and(|r| r.exit_requested) {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = self.running.as_mut() else { return };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                let width = new_size.width.max(1);
                let height = new_size.height.max(1);
                if (width, height) != r.hw.size {
                    r.surface_config.width = width;
                    r.surface_config.height = height;
                    r.surface.configure(&r.device, &r.surface_config);
                    r.hw.size = (width, height);

                    // Старая поверхность блитится растянутой, пока владелец
                    // не приаттачит новую нужного размера.
                    publish_window_resized(&r.app_pub, &r.hw.owner, width, height, r.effective_scale(), r.format_proto);
                }
                r.window.request_redraw();
            }
            WindowEvent::RedrawRequested => r.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                r.cursor_pos = (position.x as f32, position.y as f32);
                r.cursor_dirty = true;
            }
            WindowEvent::MouseInput { state: button_state, button, .. } => {
                let b_idx = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                publish_ui_event(&r.app_pub, &r.hw.owner, app::ui_event::Event::Click(
                    app::ClickEvent {
                        button: b_idx,
                        pressed: button_state == winit::event::ElementState::Pressed,
                        x: r.cursor_pos.0,
                        y: r.cursor_pos.1,
                    },
                ));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                publish_ui_event(&r.app_pub, &r.hw.owner, app::ui_event::Event::Scroll(
                    app::ScrollEvent {
                        delta_x: match delta { winit::event::MouseScrollDelta::LineDelta(x, _) => x * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.x as f32 },
                        delta_y: match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y * 120.0, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 },
                    },
                ));
            }
            WindowEvent::ModifiersChanged(m) => {
                r.key_modifiers = m.state();
            }
            WindowEvent::KeyboardInput { event: input, .. } => {
                if let winit::keyboard::PhysicalKey::Code(kc) = input.physical_key {
                    let pressed = input.state == winit::event::ElementState::Pressed;
                    publish_ui_event(&r.app_pub, &r.hw.owner, app::ui_event::Event::Key(
                        app::KeyEvent {
                            key_code: kc as u32,
                            pressed,
                            // Текст есть только у нажатий печатных клавиш.
                            text: if pressed { input.text.as_ref().map(|t| t.to_string()).unwrap_or_default() } else { String::new() },
                            modifiers: modifiers_bits(r.key_modifiers),
                        },
                    ));
                }
            }
            _ => (),
        }
    }
}

fn main() -> anyhow::Result<()> {
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

    // Рантайм заводится руками, а не `#[tokio::main]`: цикл событий винита
    // забирает поток себе и вызывает нас синхронно, поэтому асинхронная
    // инициализация ждётся через `block_on` уже изнутри цикла. `enter` держит
    // контекст рантайма на этом потоке — чтобы `spawn` из обработчиков событий
    // находил, куда планировать.
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App {
        runtime: &runtime,
        host_config,
        owner_name,
        win_cfg,
        running: None,
    })?;
    Ok(())
}
