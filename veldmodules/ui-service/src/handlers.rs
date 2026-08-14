use crate::proto::ui_service::*;
use crate::module::state::{PluginUiState, State};
use veldsdk::proto::app as app_proto;
use crate::module::renderer::GpuRenderer;
use crate::module::converter;
use crate::module::converter::UiMessage;
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

    // Владелец окна делегирует поверхность (set_surface) в ответ на
    // app/window_resized ещё до app/ready, так что к первому set_view его
    // состояние обычно уже существует с реальным размером холста.
    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);

    // Разметка запоминается, а рисуется ниже — и потом ещё на кадровых тиках:
    // статичный view диффов не даёт, и без тиков не перерисовался бы никогда.
    if let Some(layout) = req.layout {
        veldsdk::log::info!(target: "handlers", "Новый view от '{}'", plugin_id);
        plugin.set_layout(layout);
    }

    render_plugin_if_needed(state, &plugin_id);
}

/// Рисует плагин, если ему есть что показать и есть куда. Общая точка для
/// смены разметки (`handle_set_view`) и для кадрового тика (`handle_ui_event`):
/// разметка может не меняться неделю, а ввод обрабатывать всё равно нужно.
fn render_plugin_if_needed(state: &mut State, plugin_id: &str) {
    // plugins и renderer — разные поля State, заимствуются одновременно.
    let Some(plugin) = state.plugins.get_mut(plugin_id) else { return };
    // Смену разметки отдельно не проверяем: везде, где ставится
    // `is_layout_dirty`, поднимается и этот флаг (set_layout, кадровый тик).
    let needs_render = plugin.needs_redrawing;

    if let (Some(handle), true) = (plugin.surface_handle, needs_render) {
        if let Err(e) = render_plugin(plugin, &mut state.renderer, plugin_id, handle) {
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
///
/// Формат приезжает вместе с текстурой и остаётся при ней: пайплайн собирается
/// под него, а сменится текстура — сменится и он.
pub fn handle_set_surface(state: &mut State, req: veldsdk::proto::core::SurfaceDelegated) {
    let Some(plugin_id) = client("set_surface") else { return };
    // Пустая поверхность — отзыв: рисовать этому модулю больше некуда. Его
    // разметку и кэш держим — она принадлежит ему, а не месту, и вернувшись со
    // своей поверхностью он продолжит с того же состояния.
    let Some(surface) = req.surface else {
        veldsdk::log::info!(target: "handlers", "Поверхность '{}' отозвана", plugin_id);
        if let Some(plugin) = state.plugins.get_mut(&plugin_id) {
            plugin.surface_handle = None;
        }
        return;
    };
    veldsdk::log::info!(target: "handlers", "Поверхность '{}': текстура {} ({}x{})", plugin_id, surface.id, req.width, req.height);

    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);
    plugin.canvas_size = (req.width, req.height);
    plugin.scale_factor = if req.scale_factor > 0.0 { req.scale_factor } else { 1.0 };
    plugin.surface_handle = Some(surface.id);
    // Пайплайн собран под прежний формат: сменился он — пересобирать.
    if plugin.surface_format != req.format {
        plugin.surface_format = req.format;
        plugin.ui_pipeline = None;
    }
    // Кэш view привязан к texture_id и инвалидируется его сменой.
    plugin.needs_redrawing = true;

    render_plugin_if_needed(state, &plugin_id);
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
        render_plugin_if_needed(state, &plugin_id);
    }
}

/// Сырая дельта окна в скорость прокрутки: степень сильно сжимает разброс,
/// множитель задаёт размах. Так один щелчок колеса и рывок тачпада получают
/// сравнимый ход, хотя приезжают числами разного порядка.
fn smooth(delta: f32) -> f32 {
    delta.signum() * delta.abs().powf(SCROLL_CURVE) * SCROLL_REACH
}

const SCROLL_CURVE: f32 = 1.0 / 6.0;
const SCROLL_REACH: f32 = 24.0;

/// Один щелчок колеса — 120 единиц: так его называет окно (`main.rs`), и так
/// его называет всякая оконная система со времён Windows.
const RAW_WHEEL_NOTCH: f32 = 120.0;

/// Во столько пикселей сглаженного потока обходится один щелчок колеса.
///
/// Это ровно вся сумма приращений, которую выдаст инерция: трение гасит
/// скорость, ничего к ней не добавляя. Величина считается здесь, рядом со
/// сглаживанием: ей меряется прокрутка снаружи (см. `Viewport`), и названная
/// где-нибудь ещё она отстала бы от него молча.
pub fn wheel_notch() -> f32 {
    smooth(RAW_WHEEL_NOTCH)
}

fn process_ui_event(plugin: &mut PluginUiState, plugin_id: &str, req_event: app_proto::UiEvent) {
    let Some(ev) = req_event.event else { return };

    match ev {
        app_proto::ui_event::Event::Scroll(s) => {
            let vel = &mut plugin.scroll_velocity;
            if (s.delta_y > 0.0 && vel.y < 0.0) || (s.delta_y < 0.0 && vel.y > 0.0) {
                vel.y = 0.0;
            }

            vel.x = (vel.x + smooth(s.delta_x)).clamp(-3000.0, 3000.0);
            vel.y = (vel.y + smooth(s.delta_y)).clamp(-3000.0, 3000.0);
        }
        app_proto::ui_event::Event::Frame(f) => {
            plugin.monitor_fps = f.monitor_fps;

            // Темп разбора кадров. Отладочная величина, поэтому debug: в
            // консоли ей делать нечего, а в trace.log она попадёт.
            if let Some(report) = plugin.frames.tick() {
                veldsdk::log::debug!(target: "perf", "{}: {}", plugin_id, report);
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
            //
            // Живая текстура в прошлом кадре — тоже повод рисовать: её владелец
            // перерисовывает её сам, а наша разметка при этом не меняется, и по
            // ней об устаревании показанного не узнать.
            if !plugin.pending_events.is_empty()
                || plugin.is_layout_dirty
                || GpuRenderer::has_live_image(&plugin.last_draw_commands)
            {
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
    veldsdk::log::trace!(target: "handlers", "UI message -> '{}/{}'", plugin_id, message.method);
    crate::emit::on_ui_event(&message, plugin_id);
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
            let button = crate::module::pointer::from_index(c.button)?;
            Some(if c.pressed { Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)) }
                 else { Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)) })
        }
        _ => None,
    }
}

fn cursor_at(position: Point) -> iced_core::mouse::Cursor {
    iced_core::mouse::Cursor::Available(position)
}

fn render_plugin(plugin: &mut PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, target_texture: u64) -> anyhow::Result<()> {
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
    // Кончился ли в этом кадре перенос. Считается до раздачи событий: сбросить
    // несомое должен кадровый цикл, а не виджеты, — отпускание видят и взявший,
    // и принявший, а порядок между ними не определён (см. drag.rs).
    let released = events.iter().any(|event| {
        matches!(
            event,
            Event::Mouse(iced_core::mouse::Event::ButtonReleased(iced_core::mouse::Button::Left))
        )
    });
    events.push(Event::Window(iced_core::window::Event::RedrawRequested(std::time::Instant::now())));
    veldsdk::log::trace!(target: "render", "Processing {} events", events.len());

    let sf = plugin.scale_factor;
    renderer.update_params(width, height, sf);
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

    // Курсор iced получает параметром `update`, а не вычитывает из событий,
    // поэтому вся переданная пачка видит одну позицию. Отдать её разом значит
    // оценить нажатие там, где курсор оказался к концу кадра: за кадр он
    // успевает уехать, и под ним к этому моменту уже другая кнопка.
    //
    // Поэтому пачка режется по движениям: каждый отрезок идёт со своей
    // позицией, а само движение — с новой, вместе с тем, что за ним следует.
    let mut at = plugin.cursor_position;
    let mut batch: Vec<Event> = Vec::new();
    for event in events {
        if let Event::Mouse(iced_core::mouse::Event::CursorMoved { position }) = event {
            if !batch.is_empty() {
                let _ = ui.update(&batch, cursor_at(at), renderer, &mut clipboard, &mut captured_messages);
                batch.clear();
            }
            at = position;
        }
        batch.push(event);
    }
    let _ = ui.update(&batch, cursor_at(at), renderer, &mut clipboard, &mut captured_messages);
    // Позиция переживает кадр: следующий начинается там, где кончился этот, а
    // движения может и не быть вовсе.
    plugin.cursor_position = at;

    // Отпустили — значит несомое доставлено (или уронено мимо зон): подсветка
    // гаснет уже в этом кадре, до отрисовки.
    if released {
        crate::module::drag::drop_carried();
    }

    // Наводка — после ввода и до отрисовки: колесо этого кадра уже учтено, а
    // нарисуется область уже там, куда её навели.
    aim_scrollables(plugin, &mut ui, renderer);

    veldsdk::log::trace!(target: "render", "Drawing UI");
    ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor_at(at));

    // Геометрия кадра сверяется с прошлой: разметка могла не измениться, а
    // нарисоваться иначе — от наведения, каретки или инерции прокрутки.
    //
    // Живая текстура из-под этой проверки выведена, и обойти проверку —
    // единственный способ её показать: геометрия у неё как раз и совпадает от
    // кадра к кадру, а меняется то, что внутри неё. Пропусти мы кадр — в окне
    // остался бы прошлый композит, и вид, который владелец продолжает рисовать,
    // замер бы на экране.
    let commands_changed = plugin.last_draw_commands != renderer.draw_commands
        || plugin.last_vertices != renderer.vertices
        || renderer.is_atlas_dirty();
    let has_live = GpuRenderer::has_live_image(&renderer.draw_commands);
    veldsdk::log::trace!(target: "render", "commands_changed={}, is_layout_dirty={}, live={}", commands_changed, plugin.is_layout_dirty, has_live);

    if commands_changed || plugin.is_layout_dirty || has_live {
        veldsdk::log::trace!(target: "render", "Rendering into target texture {}", target_texture);
        crate::module::graphics::render_ui(plugin, renderer, target_texture, width, height, sf)?;

        // Снимок кадра для диффа — обменом, а не клонированием: рендерер
        // пересобирает геометрию каждый кадр с нуля (clear в начале сборки),
        // а клонировать тысячи вершин на каждом изменившемся кадре дорого.
        std::mem::swap(&mut plugin.last_draw_commands, &mut renderer.draw_commands);
        std::mem::swap(&mut plugin.last_vertices, &mut renderer.vertices);
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

/// Наводит области прокрутки туда, куда просит разметка.
///
/// Один раз на просьбу: разметка пересобирается на каждое событие, и наводка,
/// применяемая каждый кадр, держала бы прокрутку намертво — колесо крутилось
/// бы, а список стоял. Помнится поэтому номер просьбы, а не смещение: к одной
/// и той же строке приводят дважды подряд, и по смещению вторую просьбу от
/// первой не отличить.
///
/// Забытая наводка (`scroll_to` пропал) снимается с учёта: клиент увёл строку с
/// глаз сам. Область, ушедшая из разметки, — тоже: переключили вкладку, и,
/// вернувшись на неё, к строке надо привести заново — состояние прокрутки за
/// это время пересборки не пережило.
fn aim_scrollables(plugin: &mut PluginUiState, ui: &mut UserInterface<'_, UiMessage, Theme, GpuRenderer>, renderer: &GpuRenderer) {
    let mut asked: Vec<(String, Option<ScrollTo>)> = Vec::new();
    collect_aims(plugin.layout.root.as_ref(), &mut asked);
    plugin.aimed.retain(|name, _| asked.iter().any(|(shown, _)| shown == name));

    for (name, aim) in asked {
        let Some(aim) = aim else {
            plugin.aimed.remove(&name);
            continue;
        };
        if plugin.aimed.get(&name) == Some(&aim.request) {
            continue;
        }
        plugin.aimed.insert(name.clone(), aim.request);
        veldsdk::log::debug!(target: "render", "наводка '{}' на {:.0}", name, aim.offset);
        let mut operation = iced_core::widget::operation::scrollable::scroll_to(
            iced_core::widget::Id::from(name.clone()),
            iced_core::widget::operation::scrollable::AbsoluteOffset {
                x: None,
                y: Some(aim.offset),
            },
        );
        ui.operate(renderer, &mut operation);
    }
}

/// Все именованные области прокрутки разметки и то, куда их просят навести.
///
/// Match исчерпывающий намеренно: обход знает форму дерева отдельно от
/// `converter::convert_widget`, и завести носителя детей, забыв про него здесь,
/// значило бы молча потерять наводку у всего, что внутри него. С исчерпывающим
/// match'ем об этом скажет компилятор.
fn collect_aims(widget: Option<&Widget>, into: &mut Vec<(String, Option<ScrollTo>)>) {
    let Some(widget) = widget else { return };
    let Some(kind) = &widget.r#type else { return };
    match kind {
        widget::Type::Scrollable(scroll) => {
            if !scroll.name.is_empty() {
                into.push((scroll.name.clone(), scroll.scroll_to.clone()));
            }
            collect_aims(scroll.content.as_deref(), into);
        }
        widget::Type::Column(column) => {
            for child in &column.children {
                collect_aims(Some(child), into);
            }
        }
        widget::Type::Row(row) => {
            for child in &row.children {
                collect_aims(Some(child), into);
            }
        }
        widget::Type::Container(container) => collect_aims(container.child.as_deref(), into),
        widget::Type::Tooltip(tooltip) => collect_aims(tooltip.content.as_deref(), into),
        widget::Type::Popover(popover) => {
            collect_aims(popover.anchor.as_deref(), into);
            collect_aims(popover.panel.as_deref(), into);
        }
        // Детей не носят — идти внутрь некуда.
        widget::Type::Text(_)
        | widget::Type::TextInput(_)
        | widget::Type::Image(_)
        | widget::Type::Space(_)
        | widget::Type::ProgressBar(_)
        | widget::Type::Slider(_)
        | widget::Type::Divider(_)
        | widget::Type::Viewport(_) => {}
    }
}
