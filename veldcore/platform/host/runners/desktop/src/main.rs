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
    /// Композитируемая поверхность: bind group для блита.
    /// None до первого set_surface — окно рисует фоновый цвет.
    surface: Option<wgpu::BindGroup>,
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
    /// Раннер не поднялся. Отдельным полем, а не отсутствием `running`: без
    /// окна прогон не состоялся вовсе, и снаружи это обязано быть отказом.
    broken: bool,
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
    /// Незакрытое ожидание сценария; `None` — сценарий идёт своим ходом.
    awaiting: Option<Awaited>,
    /// Номер последнего заданного вопроса. Ответ на прежний — не к нам: экран
    /// за это время сменился, и место в нём уже не то.
    asked: u64,
    /// Предел ожидания для шагов сценария; меняется шагом `timeout`.
    patience: std::time::Duration,
    /// Сценарий не сошёлся с тем, что на экране. Прогон кончится отказом —
    /// иначе провалившаяся проверка выглядела бы как успешный запуск.
    failed: bool,
}

/// Шаг сценария, ждущий ответа рендерера: где на экране названный элемент.
struct Awaited {
    address: capture::Address,
    /// Нажать, как только дождёмся.
    tap: bool,
    /// Ждём, что элемента не станет.
    gone: bool,
    /// Спрашивать ли снова. Нет — у шага один ответ, и не сошедшийся ответ
    /// сразу валит прогон (`expect`/`absent`).
    retry: bool,
    /// Когда сдаваться.
    until: std::time::Instant,
    /// Вопрос в полёте; пока он есть, второго не задаём.
    in_flight: Option<u64>,
}

/// Сколько ждём один ответ рендерера. Обход стои́т кадра, а кадр — вертикальной
/// синхронизации; молчание дольше этого значит, что рисовать уже некому.
const ANSWER: std::time::Duration = std::time::Duration::from_secs(2);

/// Предел ожидания по умолчанию: список каталога приезжает из сети, и
/// пятнадцати секунд ему бывает мало.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// Один щелчок колеса в единицах, которыми его считает окно. По ту сторону
/// провода то же число знает ui-service (`RAW_WHEEL_NOTCH`); равенство держит
/// таблица пар `buildgen/tests/test_wire_pairs.py`.
const WHEEL_NOTCH: f32 = 120.0;

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
        log::info!(target: "render", "Поднимаем wgpu (только Vulkan)...");
        // Валидация Vulkan — только по запросу через env (WGPU_VALIDATION=1 и т.п.):
        // `InstanceFlags::all()` включил бы её и в релизе, а полный validation
        // layer стоит кадрового бюджета.
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
        let (_adapter, device, queue, surface_config, surface_format, faults) =
            veldmap_host_core::setup::init_wgpu(&instance, &surface, size.width, size.height).await?;

        // ── 3. Ядро и сервисы ──────────────────────────────────────────────
        let ctx = veldmap_host_core::setup::init_core_services(
            device.clone(), queue.clone(), surface_format, host_config, faults,
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
            anyhow::bail!("Владелец окна '{}' — не загруженный сервис", owner_name);
        }

        let compositor = Compositor::new(&device, surface_format);
        let format_proto = ctx.graphics.get_surface_format_proto();
        // Снимки ложатся туда же, где логи: и то и другое — про последний запуск.
        let script = capture::Script::from_env(
            ctx.config.log_path().parent().unwrap_or(std::path::Path::new(".")),
        )?;

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
            awaiting: None,
            asked: 0,
            patience: PATIENCE,
            failed: false,
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

        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::Frame(app::FrameEvent { dt }));

        // Атомарный свап поверхности, если владелец приаттачил новую.
        // Права проверены на входе (модуль app + фасад Surfaces);
        // здесь остаётся только подмена между кадрами.
        if let Some(texture_id) = self.ctx.surfaces.take(&self.hw.owner) {
            match self.ctx.memory.get_texture(texture_id) {
                Some((texture, _, _, _)) => {
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind_group = self.compositor.create_bind_group(&self.device, &view);
                    self.hw.surface = Some(bind_group);
                    log::debug!(target: "render", "К окну '{}' подключена поверхность: текстура {}", self.hw.owner, texture_id);
                }
                None => log::warn!(target: "render", "set_surface для '{}' назвал неизвестную текстуру {}", self.hw.owner, texture_id),
            }
        }

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            // Suboptimal — кадр годный, но свопчейн разошёлся с окном; его
            // рисуем и переконфигурируем со следующего.
            Acquired::Success(f) | Acquired::Suboptimal(f) => f,
            Acquired::Lost | Acquired::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                log::debug!(target: "render", "Поверхность окна перенастроена после потери");
                self.window.request_redraw();
                return;
            }
            // Таймаут и перекрытое окно — пропуск кадра без перенастройки.
            // Просьбу нарисовать снова при этом надо повторить: цикл событий
            // ждёт (`ControlFlow::Wait`), а весь его завод — та самая просьба
            // в конце `redraw`, до которой этот пропуск не доходит. Без неё
            // приложение замирает насовсем и молча: ни кадров, ни ввода, ни
            // строки в логе — а вместе с кадрами встают и часы прогона по
            // сценарию.
            other => {
                log::debug!(target: "render", "Кадр пропущен: поверхность вернула {:?}", other);
                self.window.request_redraw();
                return;
            }
        };
        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // 1) Render-опы модулей — в их таргеты (обычно текстура окна).
        // Опы приходят уже разрешёнными: аттачменты и объекты команд проверены
        // на submit и живы — их держит сам op. Здесь остаётся открыть pass.
        let graphics = self.ctx.graphics.clone();
        for op in graphics.take_pending_ops() {
            let at = &op.attachments;
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Module Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &at.target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                // Глубина очищается в дальнюю плоскость: кадр рисуется с нуля,
                // как и цвет. Трафарета у буфера нет — формат бесстенсильный.
                depth_stencil_attachment: at.depth.as_ref().map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }
                }),
                ..Default::default()
            });
            veldmap_host_core::graphics::execute_render_commands(
                &mut rp, &op.commands, at.width, at.height,
            );
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
            if let Some(bind_group) = &self.hw.surface {
                self.compositor.blit_ui(&mut rp, bind_group);
            }
        }

        // Отправка идёт под захватом очереди: сабмитят в неё и чтения ресурсов
        // с blocking-пула, а порядок между ними важен.
        //
        // Показ — уже без захвата, и это не мелочь: при vsync он ждёт развёртки,
        // то есть почти весь кадр. Под тем же замком это ожидание встаёт поперёк
        // любой записи плагина в GPU-буфер (`MemoryManager::write` берёт очередь
        // ради `write_buffer`), и модуль перестаёт успевать за кадровым циклом —
        // а очереди шины неограниченные, поэтому отставание не теряется, а
        // копится, и ввод доезжает секундами позже.
        //
        // Порядок кадров от захвата не зависит: в свопчейн пишет только этот
        // цикл, и он однопоточный, так что чужому submit между отправкой и
        // показом взяться неоткуда.
        let queue = {
            let queue = self.queue.lock().unwrap();
            queue.submit(Some(encoder.finish()));
            queue.clone()
        };
        queue.present(frame);

        self.play_script();

        // Модули рендерят в ответ на Frame-события: цикл кадров живёт
        // на хосте (темп задаёт present_mode/vsync).
        self.window.request_redraw();
    }

    /// Левая кнопка сценария там, где сейчас стоит его курсор.
    fn publish_button(&self, pressed: bool) {
        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::Click(
            app::ClickEvent { button: 1, pressed },
        ));
    }

    /// Курсор сценария — сразу, а не коалесцированно.
    ///
    /// Коалесценция заведена под поток движений от окна (40–125 в секунду), а
    /// сценарий движет курсор считаное число раз. Отложи мы его до следующего
    /// кадра — слипшиеся в один кадр `move` и `click` дали бы нажатие по
    /// прежней точке: iced берёт её из последнего движения.
    fn move_cursor(&mut self, at: (f32, f32)) {
        self.cursor_pos = at;
        self.cursor_dirty = false;
        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::CursorMoved(
            app::CursorMovedEvent { x: at.0, y: at.1 },
        ));
    }

    /// Клавиша целиком: код, порождённый ею текст и состояние.
    fn publish_key(&self, code: u32, text: String, pressed: bool) {
        publish_ui_event(&self.app_pub, &self.hw.owner, app::ui_event::Event::Key(
            app::KeyEvent { key_code: code, pressed, text, modifiers: 0 },
        ));
    }

    /// Набор текста: по знаку за нажатие, туда, где стоит каретка.
    ///
    /// Физический код у набранного свой и заведомо не совпадающий ни с одной
    /// клавишей: раскладка к этому времени уже применена, и рендереру нужен
    /// текст, а не то, чем его набрали (см. ui-service/keyboard.rs).
    fn publish_typing(&self, text: &str) {
        const TYPED: u32 = u32::MAX;
        for symbol in text.chars() {
            self.publish_key(TYPED, symbol.to_string(), true);
            self.publish_key(TYPED, String::new(), false);
        }
    }

    /// Отыгрывает шаги отладочного сценария, чей срок настал. Идёт после
    /// показа кадра: снимок обязан застать уже отрисованное состояние, а не
    /// то, что было до render-опов этого кадра.
    fn play_script(&mut self) {
        // Незакрытое ожидание идёт первым и один занимает кадр: часы сценария
        // на это время стоят, и следующим шагам всё равно не время.
        if self.awaiting.is_some() {
            self.settle();
            return;
        }
        let Some(script) = &mut self.script else { return };
        let due = script.due();
        for action in due {
            match action {
                capture::Action::Move { x, y } => self.move_cursor((x, y)),
                capture::Action::Click => {
                    self.publish_button(true);
                    self.publish_button(false);
                }
                capture::Action::Button { pressed } => self.publish_button(pressed),
                capture::Action::Scroll { dx, dy } => publish_ui_event(
                    &self.app_pub,
                    &self.hw.owner,
                    app::ui_event::Event::Scroll(app::ScrollEvent { delta_x: dx, delta_y: dy }),
                ),
                capture::Action::Shot { path } => {
                    let queue = self.queue.lock().unwrap();
                    let frame = capture::FrameSource {
                        device: &self.device,
                        queue: &queue,
                        compositor: &self.compositor,
                        surface: self.hw.surface.as_ref(),
                        size: self.hw.size,
                        format: self.surface_config.format,
                    };
                    match capture::shoot(frame, &path) {
                        Ok(()) => log::info!(target: "render", "Снимок: {}", path.display()),
                        Err(e) => log::error!(target: "render", "Снимок '{}' не сделан: {:#}", path.display(), e),
                    }
                }
                capture::Action::Patience { limit } => self.patience = limit,
                capture::Action::Type { text } => {
                    log::info!(target: "render", "Сценарий: набрано «{}»", text);
                    self.publish_typing(&text);
                }
                capture::Action::Key { code, name } => {
                    log::info!(target: "render", "Сценарий: клавиша {}", name);
                    self.publish_key(code, String::new(), true);
                    self.publish_key(code, String::new(), false);
                }
                capture::Action::Tap { address } => self.await_widget(address, true, false, true),
                capture::Action::Await { address, gone } => self.await_widget(address, false, gone, true),
                capture::Action::Assert { address, gone } => self.await_widget(address, false, gone, false),
                capture::Action::Exit => self.exit_requested = true,
            }
        }
    }

    /// Заводит ожидание: спрашивает рендерера, где элемент, и останавливает
    /// часы сценария до ответа.
    fn await_widget(&mut self, address: capture::Address, tap: bool, gone: bool, retry: bool) {
        if let Some(script) = &mut self.script {
            script.hold();
        }
        // Срок у переспрашивающего — терпение сценария, у однократного — время
        // на один вопрос-ответ: он спрашивает про то, что видно сейчас, и
        // ждать ему нечего, кроме самого ответа.
        let limit = match retry {
            true => self.patience,
            false => ANSWER,
        };
        let in_flight = Some(self.ask(&address));
        self.awaiting = Some(Awaited {
            address,
            tap,
            gone,
            retry,
            until: std::time::Instant::now() + limit,
            in_flight,
        });
    }

    /// Спрашивает рендерера, где на экране элемент, и возвращает номер вопроса.
    fn ask(&mut self, address: &capture::Address) -> u64 {
        self.asked += 1;
        app_bus::emit::on_locate_widget(&self.app_pub, &app::LocateWidget {
            plugin_id: self.hw.owner.clone(),
            request: self.asked,
            method: address.method.clone(),
            value: address.value.clone(),
            text: address.text.clone(),
            ordinal: address.ordinal,
        });
        self.asked
    }

    /// Двигает незакрытое ожидание: забирает ответ, решает, дождались ли, и
    /// либо продолжает сценарий, либо валит прогон.
    fn settle(&mut self) {
        let answer = self.ctx.places.take(&self.hw.owner);
        let Some(mut waiting) = self.awaiting.take() else { return };

        // Ответ на позапрошлый вопрос отбрасываем молча: экран с тех пор
        // сменился, и место в нём уже не то.
        if let Some(place) = answer.filter(|place| Some(place.request) == waiting.in_flight) {
            waiting.in_flight = None;

            // Несколько подошедших под безномерный адрес — ошибка сценария, а
            // не ожидания: ждать, пока лишние исчезнут, бессмысленно.
            if !waiting.gone && waiting.address.ordinal == 0 && place.found > 1 {
                self.fail(format!(
                    "«{}»: подошло {} — назовите номером, который из них нужен",
                    waiting.address, place.found
                ));
                return;
            }

            // «Есть» и «нет» — не отрицания друг друга: безномерный адрес
            // считается найденным ровно у одного, а пропавшим — у нуля.
            // Иначе два одинаковых на экране сошлись бы и как «не нашёлся», и
            // как «пропал».
            let sated = match (waiting.gone, waiting.address.ordinal) {
                (false, 0) => place.found == 1,
                (false, n) => place.found >= n,
                (true, 0) => place.found == 0,
                (true, n) => place.found < n,
            };
            if sated {
                // Место годно, только если в него можно попасть курсором:
                // вырожденный прямоугольник или уехавший за окно дал бы
                // нажатие мимо, а в лог — бодрое «нажат».
                if waiting.tap && !self.reachable(&place) {
                    self.fail(format!(
                        "«{}»: место {}×{} в точке {}×{} — курсором туда не попасть",
                        waiting.address, place.width, place.height, place.x, place.y
                    ));
                    return;
                }
                if waiting.tap {
                    self.tap_at(&place);
                }
                log::info!(target: "render", "Сценарий: «{}» — {}",
                    waiting.address,
                    match (waiting.gone, waiting.tap) {
                        (true, _) => "пропал",
                        (_, true) => "нажат",
                        _ => "на экране",
                    });
                if let Some(script) = &mut self.script {
                    script.resume();
                }
                return;
            }

            // Ответ пришёл и не сошёлся. Переспрашивающий подождёт ещё, а
            // однократному это и есть приговор.
            if !waiting.retry {
                self.fail(format!(
                    "«{}»: {}",
                    waiting.address,
                    match waiting.gone {
                        true => format!("всё ещё на экране ({})", place.found),
                        false => "на экране нет".to_string(),
                    }
                ));
                return;
            }
        }

        if std::time::Instant::now() >= waiting.until {
            self.fail(match waiting.in_flight {
                Some(_) => format!("«{}»: рендерер не ответил", waiting.address),
                None => format!("«{}»: не дождались", waiting.address),
            });
            return;
        }
        if waiting.in_flight.is_none() {
            waiting.in_flight = Some(self.ask(&waiting.address));
        }
        self.awaiting = Some(waiting);
    }

    /// Нажать в середину видимой части элемента.
    ///
    /// Курсор переезжает по-настоящему и отдельным событием до нажатия: iced
    /// берёт точку нажатия из последнего движения, а коалесцированный `move`
    /// раннера уехал бы только следующим кадром — то есть уже после клика.
    fn tap_at(&mut self, place: &veldmap_host_core::places::Place) {
        self.move_cursor((place.x + place.width / 2.0, place.y + place.height / 2.0));
        self.publish_button(true);
        self.publish_button(false);
    }

    /// Оборвали ли прогон раньше, чем сценарий кончился.
    fn unfinished(&self) -> bool {
        self.script.as_ref().is_some_and(|script| script.unfinished()) || self.awaiting.is_some()
    }

    /// Достанет ли курсор до середины названного места.
    fn reachable(&self, place: &veldmap_host_core::places::Place) -> bool {
        let (width, height) = self.hw.size;
        let (x, y) = (place.x + place.width / 2.0, place.y + place.height / 2.0);
        place.width > 0.0
            && place.height > 0.0
            && (0.0..width as f32).contains(&x)
            && (0.0..height as f32).contains(&y)
    }

    /// Сценарий не сошёлся с экраном: досматривать нечего, а прогон обязан
    /// кончиться отказом.
    fn fail(&mut self, why: String) {
        log::error!(target: "render", "Сценарий не сошёлся: {}", why);
        self.failed = true;
        self.exit_requested = true;
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
                log::error!(target: "render", "Раннер не запустился: {:#}", e);
                self.broken = true;
                event_loop.exit();
            }
        }
    }

    /// Цикл событий закрывается — рантайм сейчас разберут.
    ///
    /// Объявляется это здесь, в самый ранний момент, какой есть: на
    /// blocking-пуле может доживать оконное чтение удалённого ресурса, отменить
    /// которое нечем — оно живёт под синхронным ABI памяти, а не под задачей, —
    /// и в сеть ему теперь нельзя (см. `veldmap_host_core::shutting_down`).
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        veldmap_host_core::begin_shutdown();
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
                    },
                ));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                publish_ui_event(&r.app_pub, &r.hw.owner, app::ui_event::Event::Scroll(
                    app::ScrollEvent {
                        delta_x: match delta { winit::event::MouseScrollDelta::LineDelta(x, _) => x * WHEEL_NOTCH, winit::event::MouseScrollDelta::PixelDelta(p) => p.x as f32 },
                        delta_y: match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y * WHEEL_NOTCH, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 },
                    },
                ));
            }
            // Окно переехало на экран с другим DPI. Обычно следом придёт и
            // `Resized` — винит по умолчанию меняет размер окна на предложенный
            // системой, — но обязан он этого не быть: масштаб сменился уже
            // сейчас, а размер может и остаться прежним. Публикуем сами, мимо
            // проверки на смену размера: она бы это и съела.
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let scale = (scale_factor as f32).max(r.ui_scale);
                publish_window_resized(&r.app_pub, &r.hw.owner, r.hw.size.0, r.hw.size.1, scale, r.format_proto);
                r.window.request_redraw();
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
        1 => declared.into_iter().next().expect("длина проверена"),
        0 => anyhow::bail!("Окна не объявил ни один модуль — настольному раннеру нечего показывать"),
        n => anyhow::bail!("окна объявили {} модулей, а настольный раннер ведёт ровно одно", n),
    };
    log::info!(target: "render", "Окно '{}': владелец '{}'", win_cfg.title, owner_name);

    // Рантайм заводится руками, а не `#[tokio::main]`: цикл событий винита
    // забирает поток себе и вызывает нас синхронно, поэтому асинхронная
    // инициализация ждётся через `block_on` уже изнутри цикла. `enter` держит
    // контекст рантайма на этом потоке — чтобы `spawn` из обработчиков событий
    // находил, куда планировать.
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        runtime: &runtime,
        host_config,
        owner_name,
        win_cfg,
        running: None,
        broken: false,
    };
    event_loop.run_app(&mut app)?;

    // Прогон обязан кончиться так, как написано в сценарии, и всякий другой
    // конец — отказ: снаружи «прошло» читают по коду возврата, а не по логу,
    // и молчаливый ноль объявил бы непроверенное проверенным.
    if app.broken {
        anyhow::bail!("раннер не запустился — см. runtime/logs/host.log");
    }
    if let Some(running) = &app.running {
        if running.failed {
            anyhow::bail!("сценарий не сошёлся с тем, что на экране — см. runtime/logs/host.log");
        }
        if running.unfinished() {
            anyhow::bail!("прогон оборван раньше конца сценария — см. runtime/logs/host.log");
        }
    }
    Ok(())
}
