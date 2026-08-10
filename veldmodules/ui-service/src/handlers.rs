use crate::proto::ui_service::*;
use crate::module::state::{PluginUiState, State};
use veldsdk::proto::app as app_proto;
use crate::module::renderer::GpuRenderer;
use crate::module::converter;
use iced_core::{Point, Event, Size, Theme};
use iced_runtime::UserInterface;
use iced_graphics::Viewport;

/// Клиент текущего события — имя отправителя, которым его подписал хост. Оно
/// же ключ состояния и адрес, по которому уедут события его виджетов; в самих
/// сообщениях имени нет намеренно (см. types.proto).
///
/// `None` — событие опубликовал хост: модульной идентичности у него нет, и
/// рисовать такую разметку некому и не для кого.
fn client(topic: &str) -> Option<String> {
    let publisher = veldsdk::event_publisher();
    if publisher.is_empty() {
        veldsdk::log::warn!(target: "handlers", "{} пришёл от хоста: у разметки нет владельца", topic);
        return None;
    }
    Some(publisher)
}

pub fn handle_set_view(state: &mut State, req: SetViewRequest) {
    let Some(plugin_id) = client("set_view") else { return };
    let surface_format = state.surface_format;

    // Владелец окна делегирует поверхность (set_surface) в ответ на
    // app/window_resized ещё до app/ready, так что к первому set_view его
    // состояние обычно уже существует с реальным размером холста.
    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);

    // Разметка запоминается, а рисуется ниже — и потом ещё на кадровых тиках:
    // статичный view диффов не даёт, и без тиков не перерисовался бы никогда.
    if let Some(layout) = req.layout {
        veldsdk::log::info!(target: "handlers", "Новый view от '{}'", plugin_id);
        plugin.layout = layout;
        plugin.is_layout_dirty = true;
        plugin.needs_redrawing = true;
    }

    render_plugin_if_needed(state, &plugin_id, surface_format);
}

/// Рисует плагин, если ему есть что показать и есть куда. Общая точка для
/// смены разметки (`handle_set_view`) и для кадрового тика (`handle_ui_event`):
/// разметка может не меняться неделю, а ввод обрабатывать всё равно нужно.
fn render_plugin_if_needed(state: &mut State, plugin_id: &str, surface_format: i32) {
    // plugins и renderer — разные поля State, заимствуются одновременно.
    let Some(plugin) = state.plugins.get_mut(plugin_id) else { return };
    let needs_render = plugin.needs_redrawing || plugin.is_layout_dirty;

    if let (Some(handle), true) = (plugin.surface_handle, needs_render) {
        if let Err(e) = render_plugin(plugin, &mut state.renderer, plugin_id, surface_format, handle) {
            veldsdk::log::error!(target: "handlers", "render_plugin failed: {}", e);
        }
    }

    // Пойманное iced'ом за этот кадр (нажатия и ввод) уходит владельцу сразу.
    // Отложить до следующего set_view нельзя: его-то плагин и пришлёт в ответ
    // на эти самые сообщения — получилось бы взаимное ожидание.
    for message in std::mem::take(&mut plugin.pending_messages) {
        dispatch_event(plugin_id, message);
    }
}

/// Делегирование render-таргета владельцем окна: с этого момента ui-service
/// принимает события модуля и рендерит его view в переданную текстуру.
pub fn handle_set_surface(state: &mut State, req: SetSurfaceRequest) {
    let Some(plugin_id) = client("set_surface") else { return };
    let Some(surface) = req.surface else {
        veldsdk::log::warn!(target: "handlers", "set_surface для '{}' пришёл без поверхности", plugin_id);
        return;
    };
    veldsdk::log::info!(target: "handlers", "Поверхность '{}': текстура {} ({}x{})", plugin_id, surface.id, req.width, req.height);

    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);
    plugin.canvas_size = (req.width, req.height);
    plugin.scale_factor = if req.scale_factor > 0.0 { req.scale_factor } else { 1.0 };
    plugin.surface_handle = Some(surface.id);
    // Кэш view привязан к texture_id и инвалидируется его сменой.
    plugin.needs_redrawing = true;

    let surface_format = state.surface_format;
    render_plugin_if_needed(state, &plugin_id, surface_format);
}

pub fn handle_ui_event(state: &mut State, event_proto: app_proto::UiEvent) {
    // app/ui_event — broadcast: адресат назван в данных. Наши — только события
    // модуля с делегированной поверхностью: без неё render_plugin не зовётся,
    // а сливает pending_events он один, и ввод копился бы без потребителя.
    // Проверяем именно поверхность, а не запись в plugins: запись заводит и
    // set_view, то есть модуль с разметкой, но без поверхности её имеет.
    let plugin_id = event_proto.plugin_id.clone();
    let Some(plugin) = state.plugins.get_mut(&plugin_id) else { return };
    if plugin.surface_handle.is_none() {
        return;
    }

    let is_frame = matches!(event_proto.event, Some(app_proto::ui_event::Event::Frame(_)));
    process_ui_event(plugin, &plugin_id, event_proto);

    // Перерисовка от ввода и анимация (инерция прокрутки) — забота рендерера,
    // и обе живут кадровым тиком.
    if is_frame {
        let surface_format = state.surface_format;
        render_plugin_if_needed(state, &plugin_id, surface_format);
    }
}

fn process_ui_event(plugin: &mut PluginUiState, plugin_id: &str, req_event: app_proto::UiEvent) {
    let Some(ev) = req_event.event else { return };

    match ev {
        app_proto::ui_event::Event::Scroll(s) => {
            let vel = &mut plugin.scroll_velocity;
            if (s.delta_y > 0.0 && vel.y < 0.0) || (s.delta_y < 0.0 && vel.y > 0.0) {
                vel.y = 0.0;
            }

            let factor = 24.0;
            let dy = s.delta_y.signum() * s.delta_y.abs().powf(1.0 / 6.0) * factor;
            let dx = s.delta_x.signum() * s.delta_x.abs().powf(1.0 / 6.0) * factor;

            vel.x = (vel.x + dx).clamp(-3000.0, 3000.0);
            vel.y = (vel.y + dy).clamp(-3000.0, 3000.0);
        }
        app_proto::ui_event::Event::Frame(f) => {
            plugin.monitor_fps = f.monitor_fps;

            // FPS-счётчик: копим кадры и раз в 5 секунд отчитываемся.
            plugin.fps_window.0 += 1;
            plugin.fps_window.1 += f.dt;
            let (frames, seconds) = plugin.fps_window;
            if seconds >= 5.0 {
                veldsdk::log::info!(target: "perf", "{}: {:.1} FPS в среднем за {:.1}с", plugin_id, frames as f32 / seconds, seconds);
                plugin.fps_window = (0, 0.0);
            }

            // Инерция прокрутки.
            let velocity = plugin.scroll_velocity;
            if velocity.x.abs() > 0.1 || velocity.y.abs() > 0.1 {
                let friction = 0.92f32;
                let factor = 1.0 - friction.powf(f.dt * (plugin.monitor_fps as f32).max(60.0));
                let scrolled = Point::new(velocity.x * factor, velocity.y * factor);

                plugin.pending_events.push(Event::Mouse(iced_core::mouse::Event::WheelScrolled {
                    delta: iced_core::mouse::ScrollDelta::Pixels { x: scrolled.x, y: scrolled.y },
                }));
                plugin.scroll_velocity = Point::new(velocity.x - scrolled.x, velocity.y - scrolled.y);
            } else {
                plugin.scroll_velocity = Point::ORIGIN;
            }

            // Скопившийся ввод обрабатывается рендером этого же кадра:
            // render_plugin скармливает pending_events iced'у в ui.update().
            if !plugin.pending_events.is_empty() || plugin.is_layout_dirty {
                plugin.needs_redrawing = true;
            }
        }
        app_proto::ui_event::Event::Key(k) => {
            let mods = crate::module::keyboard::modifiers_from_bits(k.modifiers);
            // text_input хранит modifiers в собственном состоянии и обновляет
            // их только по ModifiersChanged — шлём его при смене маски.
            if plugin.keyboard_modifiers != mods {
                plugin.keyboard_modifiers = mods;
                plugin.pending_events
                    .push(Event::Keyboard(iced_core::keyboard::Event::ModifiersChanged(mods)));
            }
            plugin.pending_events
                .push(Event::Keyboard(crate::module::keyboard::convert_key_event(&k, mods)));
        }
        _ => {
            if let Some(iced_ev) = convert_event(ev, plugin.scale_factor) {
                if let Event::Mouse(iced_core::mouse::Event::CursorMoved { position }) = iced_ev {
                    plugin.cursor_position = position;
                }
                plugin.pending_events.push(iced_ev);
            }
        }
    }
}

/// Диспетчеризация захваченного виджет-события автору разметки: адресованный
/// топик `ui-service/on_ui_event`, доставляется только ему одному. Адресат
/// известен лишь в рантайме — это отправитель разметки (см. `client`).
fn dispatch_event(plugin_id: &str, message: converter::UiMessage) {
    if message.method.is_empty() {
        return;
    }
    veldsdk::log::info!(target: "handlers", "UI message -> '{}/{}' (value: '{}')", plugin_id, message.method, message.value);
    crate::emit::on_ui_event(
        &UiEventResponse { method: message.method, value: message.value },
        plugin_id,
    );
}

/// Мышь в терминах iced. Прокрутка, кадровый тик и клавиатура сюда не доходят
/// — их разбирает `process_ui_event` сам.
///
/// `None` — события такого рода у нас нет. Подставить вместо него какое-нибудь
/// нельзя: `RedrawRequested`, например, для iced не «ничего не произошло», а
/// команда зафиксировать состояние виджетов (см. `render_plugin`).
fn convert_event(ev: app_proto::ui_event::Event, sf: f32) -> Option<Event> {
    match ev {
        app_proto::ui_event::Event::CursorMoved(c) => {
            Some(Event::Mouse(iced_core::mouse::Event::CursorMoved { position: Point::new(c.x / sf, c.y / sf) }))
        }
        app_proto::ui_event::Event::Click(c) => {
            let button = match c.button { 1 => iced_core::mouse::Button::Left, 2 => iced_core::mouse::Button::Right, 3 => iced_core::mouse::Button::Middle, _ => iced_core::mouse::Button::Left };
            Some(if c.pressed { Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)) }
                 else { Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)) })
        }
        _ => None,
    }
}

fn render_plugin(plugin: &mut PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, surface_format: i32, target_texture: u64) -> anyhow::Result<()> {
    veldsdk::log::trace!(target: "render", "START for {}", plugin_id);
    let (width, height) = plugin.canvas_size;
    veldsdk::log::trace!(target: "render", "canvas size: {}x{}", width, height);
    if width == 0 || height == 0 {
        veldsdk::log::trace!(target: "render", "Empty canvas, returning");
        return Ok(());
    }

    // Кадр закрывается событием перерисовки, и без него не обойтись: виджеты
    // iced по нему запоминают своё состояние (наведение, нажатие, фокус ввода)
    // — `update` его только вычисляет, а `draw` берёт уже запомненное. Молчание
    // здесь означает «состояние неизвестно», и рисуется всё как `Disabled`:
    // кнопка без фона и рамки, вкладка голым текстом.
    //
    // Идёт последним: запоминается то состояние, до которого довёл весь
    // обработанный в этом кадре ввод.
    let mut events = std::mem::take(&mut plugin.pending_events);
    events.push(Event::Window(iced_core::window::Event::RedrawRequested(std::time::Instant::now())));
    veldsdk::log::trace!(target: "render", "Processing {} events", events.len());

    let sf = plugin.scale_factor;
    renderer.update_params(width, height, sf);
    let cursor = iced_core::mouse::Cursor::Available(plugin.cursor_position);
    let viewport = Viewport::with_physical_size(Size::new(width, height), sf.into());
    let mut captured_messages = Vec::new();

    veldsdk::log::trace!(target: "render", "Clearing renderer and converting layout");
    renderer.clear();
    let element = converter::convert_layout(&plugin.layout);

    veldsdk::log::trace!(target: "render", "Building UI");
    let cache = std::mem::take(&mut plugin.interface_cache);
    let _guard = renderer.scope_guard();

    let mut ui = UserInterface::build(
        element,
        viewport.logical_size(),
        cache,
        renderer,
    );

    veldsdk::log::trace!(target: "render", "Updating UI with {} events", events.len());
    let mut clipboard = iced_core::clipboard::Null;
    let _ = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);

    veldsdk::log::trace!(target: "render", "Drawing UI");
    ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);

    // Геометрия кадра сверяется с прошлой: разметка могла не измениться, а
    // нарисоваться иначе — от наведения, каретки или инерции прокрутки.
    let commands_changed = plugin.last_draw_commands != renderer.draw_commands
        || plugin.last_vertices != renderer.vertices
        || renderer.is_atlas_dirty();
    veldsdk::log::trace!(target: "render", "commands_changed={}, is_layout_dirty={}", commands_changed, plugin.is_layout_dirty);

    if commands_changed || plugin.is_layout_dirty {
        veldsdk::log::trace!(target: "render", "Rendering into target texture {}", target_texture);
        crate::module::graphics::render_ui(plugin, renderer, target_texture, width, height, sf, surface_format)?;

        plugin.last_draw_commands = renderer.draw_commands.clone();
        plugin.last_vertices = renderer.vertices.clone();
        plugin.is_layout_dirty = false;
    }

    plugin.needs_redrawing = false;
    veldsdk::log::trace!(target: "render", "Caching UI");
    plugin.interface_cache = ui.into_cache();

    // Отправляет их вызывающий, сразу после рендера.
    if !captured_messages.is_empty() {
        veldsdk::log::info!(target: "render", "Captured {} UI messages", captured_messages.len());
    }
    plugin.pending_messages.extend(captured_messages);

    veldsdk::log::trace!(target: "render", "END");
    Ok(())
}
