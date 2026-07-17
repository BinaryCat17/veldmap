use crate::proto::ui::*;
use crate::module::state::{PluginUiState, State, PendingMessage};
use veldsdk::rpc::app as app_proto;
use crate::module::renderer::GpuRenderer;
use crate::module::converter;
use iced_core::{Point, Event, Size, Theme};
use iced_runtime::UserInterface;
use iced_graphics::Viewport;

pub fn handle_set_view(state: &mut State, req: SetViewRequest) {
    let plugin_id = req.plugin_id.clone();
    let surface_format = state.surface_format;

    // Владелец окна делегирует поверхность (set_surface) в ответ на
    // app/window_resized ещё до app/ready, так что к первому set_view его
    // состояние обычно уже существует с реальным размером холста.
    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);

    // Store the module's current view; rendering happens below and on frame ticks.
    if let Some(layout) = req.layout {
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[SET-VIEW] '{}'", plugin_id);
        plugin.layout = layout;
        *plugin.is_layout_dirty.borrow_mut() = true;
        *plugin.needs_redrawing.borrow_mut() = true;
    }

    // plugin borrow ends here before rendering
    let _ = plugin;

    render_plugin_if_needed(state, &plugin_id, surface_format);
}

/// Render a plugin if it has pending changes and a surface handle.
/// Shared by handle_set_view (layout updates) and handle_ui_event (frame ticks):
/// a static layout produces no set_view diffs, so frame ticks must also be able
/// to trigger rendering or a never-changing UI would never be drawn.
fn render_plugin_if_needed(state: &mut State, plugin_id: &str, surface_format: i32) {
    // plugins и renderer — разные поля State, заимствуются одновременно.
    let Some(plugin) = state.plugins.get(plugin_id) else { return };
    let needs_render = *plugin.needs_redrawing.borrow() || *plugin.is_layout_dirty.borrow();
    let surface_handle = *plugin.surface_handle.borrow();

    if let (Some(handle), true) = (surface_handle, needs_render) {
        if let Err(e) = render_plugin(plugin, &mut state.renderer, plugin_id, surface_format, handle) {
            veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[render_plugin_if_needed] render_plugin failed: {}", e);
        }
    }

    // Dispatch messages captured by iced during this render (button presses etc.)
    // immediately: deferring them to the next set_view would deadlock, because the
    // plugin only sends set_view after reacting to these very messages.
    let pending = plugin.pending_messages.borrow_mut().drain(..).collect::<Vec<_>>();
    for msg in pending {
        dispatch_event(UiEventResponse {
            plugin_id: plugin_id.to_string(),
            method: msg.method,
            value: msg.value,
        });
    }
}

/// Делегирование render-таргета владельцем окна: с этого момента ui-service
/// принимает события модуля и рендерит его view в переданную текстуру.
pub fn handle_set_surface(state: &mut State, req: SetSurfaceRequest) {
    let Some(surface) = req.surface else {
        veldsdk::vwarn!(veldsdk::FLAG_UI_HANDLERS, "[SET-SURFACE] '{}' without a surface handle", req.plugin_id);
        return;
    };
    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[SET-SURFACE] '{}': texture {} ({}x{})", req.plugin_id, surface.id, req.width, req.height);

    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    *plugin.canvas_size.borrow_mut() = (req.width, req.height);
    *plugin.scale_factor.borrow_mut() = if req.scale_factor > 0.0 { req.scale_factor } else { 1.0 };
    *plugin.surface_handle.borrow_mut() = Some(surface.id);
    // Кэш view привязан к texture_id и инвалидируется его сменой.
    *plugin.needs_redrawing.borrow_mut() = true;

    let surface_format = state.surface_format;
    render_plugin_if_needed(state, &req.plugin_id, surface_format);
}

pub fn handle_ui_event(state: &mut State, event_proto: app_proto::UiEvent) {
    // app/ui_event — broadcast: адресат назван в данных. События модулей,
    // не делегировавших нам поверхность, не наши — молча пропускаем,
    // иначе их ввод копился бы в pending_events без потребителя.
    let plugin_id = event_proto.plugin_id.clone();
    if !state.plugins.contains_key(&plugin_id) {
        return;
    }

    let is_frame = matches!(event_proto.event, Some(app_proto::ui_event::Event::Frame(_)));

    if let Err(e) = process_ui_event(state, &plugin_id, event_proto) {
        veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] process_ui_event failed for {}: {}", plugin_id, e);
    }

    // On each frame tick render the plugin if it has pending changes: input-driven
    // redraws and animation (scroll inertia) are the renderer's own responsibility.
    if is_frame {
        let surface_format = state.surface_format;
        render_plugin_if_needed(state, &plugin_id, surface_format);
    }
}

fn process_ui_event(state: &mut State, plugin_id: &str, req_event: app_proto::UiEvent) -> anyhow::Result<()> {
    let plugin = state.plugins.get(plugin_id).expect("checked by caller");

    if let Some(ev) = req_event.event {
        match ev {
            app_proto::ui_event::Event::Scroll(s) => {
                let mut vel = plugin.scroll_velocity.borrow_mut();
                if (s.delta_y > 0.0 && vel.y < 0.0) || (s.delta_y < 0.0 && vel.y > 0.0) {
                    vel.y = 0.0;
                }
                
                let factor = 24.0;
                let dy = s.delta_y.signum() * s.delta_y.abs().powf(1.0 / 6.0) * factor;
                let dx = s.delta_x.signum() * s.delta_x.abs().powf(1.0 / 6.0) * factor;
                
                vel.x += dx;
                vel.y += dy;
                vel.x = vel.x.clamp(-3000.0, 3000.0);
                vel.y = vel.y.clamp(-3000.0, 3000.0);
            }
            app_proto::ui_event::Event::Frame(f) => {
                *plugin.monitor_fps.borrow_mut() = f.monitor_fps;

                // FPS-счётчик: копим кадры и раз в 5 секунд отчитываемся.
                {
                    let mut fps = plugin.fps_window.borrow_mut();
                    fps.0 += 1;
                    fps.1 += f.dt;
                    if fps.1 >= 5.0 {
                        veldsdk::vinfo!(veldsdk::FLAG_PERF, "[FPS] {}: {:.1} avg over {:.1}s", plugin_id, fps.0 as f32 / fps.1, fps.1);
                        *fps = (0, 0.0);
                    }
                }

                // Process scroll inertia
                {
                    let mut vel = plugin.scroll_velocity.borrow_mut();
                    if vel.x.abs() > 0.1 || vel.y.abs() > 0.1 {
                        let monitor_fps = *plugin.monitor_fps.borrow() as f32;
                        let friction = 0.92f32; 
                        
                        let factor = 1.0 - friction.powf(f.dt * monitor_fps.max(60.0));
                        let scroll_amount_x = vel.x * factor;
                        let scroll_amount_y = vel.y * factor;
                        
                        plugin.pending_events.borrow_mut().push(Event::Mouse(iced_core::mouse::Event::WheelScrolled { 
                            delta: iced_core::mouse::ScrollDelta::Pixels { x: scroll_amount_x, y: scroll_amount_y } 
                        }));
                        
                        vel.x -= scroll_amount_x;
                        vel.y -= scroll_amount_y;
                    } else {
                        vel.x = 0.0;
                        vel.y = 0.0;
                    }
                }

                // Скопившийся ввод обрабатывается рендером этого же кадра:
                // render_plugin скармливает pending_events iced'у в ui.update().
                if !plugin.pending_events.borrow().is_empty() || *plugin.is_layout_dirty.borrow() {
                    *plugin.needs_redrawing.borrow_mut() = true;
                }
            }
            _ => {
                let iced_ev = convert_event(ev, *plugin.scale_factor.borrow());
                if let Event::Mouse(iced_core::mouse::Event::CursorMoved { position }) = iced_ev {
                    *plugin.cursor_position.borrow_mut() = position;
                }
                plugin.pending_events.borrow_mut().push(iced_ev);
            }
        }
    }

    Ok(())
}

/// Диспетчеризация захваченного виджет-события владельцу:
/// адрес — входной метод модуля, `{plugin_id}/{method}`.
fn dispatch_event(event: UiEventResponse) {
    use veldsdk::prost::Message;
    if event.method.is_empty() {
        return;
    }
    let topic = format!("{}/{}", event.plugin_id, event.method);
    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[DISPATCH] UI message -> '{}' (value: '{}')", topic, event.value);
    veldsdk::rpc::host::publish(&topic, event.encode_to_vec());
}

fn convert_event(ev: app_proto::ui_event::Event, sf: f32) -> Event {
    match ev {
        app_proto::ui_event::Event::CursorMoved(c) => Event::Mouse(iced_core::mouse::Event::CursorMoved { position: Point::new(c.x / sf, c.y / sf) }),
        app_proto::ui_event::Event::Click(c) => {
            let button = match c.button { 1 => iced_core::mouse::Button::Left, 2 => iced_core::mouse::Button::Right, 3 => iced_core::mouse::Button::Middle, _ => iced_core::mouse::Button::Left };
            if c.pressed { Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)) }
            else { Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)) }
        }
        _ => Event::Window(iced_core::window::Event::RedrawRequested(std::time::Instant::now())),
    }
}

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, surface_format: i32, target_texture: u64) -> anyhow::Result<()> {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] START for {}", plugin_id);
    let (width, height) = *plugin.canvas_size.borrow();
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] canvas size: {}x{}", width, height);
    if width == 0 || height == 0 {
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Empty canvas, returning");
        return Ok(());
    }

    let events = std::mem::take(&mut *plugin.pending_events.borrow_mut());
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Processing {} events", events.len());

    let sf = *plugin.scale_factor.borrow();
    renderer.update_params(width, height, sf);
    let cursor_pos = *plugin.cursor_position.borrow();
    let cursor = iced_core::mouse::Cursor::Available(cursor_pos);
    let viewport = Viewport::with_physical_size(Size::new(width, height), sf.into());
    let mut captured_messages = Vec::new();

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Clearing renderer and converting layout");
    renderer.clear();
    let element = converter::convert_layout(&plugin.layout);

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Building UI");
    let cache = plugin.interface_cache.replace(iced_runtime::user_interface::Cache::default());
    let _guard = crate::module::renderer::ScopeGuard::new(&mut renderer.font_system, &mut renderer.swash_cache);

    let mut ui = UserInterface::build(
        element,
        viewport.logical_size(),
        cache,
        renderer,
    );

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Updating UI with {} events", events.len());
    let mut clipboard = iced_core::clipboard::Null;
    let _ = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Drawing UI");
    ui.draw(renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);

    let mut last_cmds = plugin.last_draw_commands.borrow_mut();
    let mut last_verts = plugin.last_vertices.borrow_mut();
    let mut is_layout_dirty = plugin.is_layout_dirty.borrow_mut();

    let commands_changed = *last_cmds != renderer.draw_commands ||
                           *last_verts != renderer.vertices ||
                           renderer.is_atlas_dirty();
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] commands_changed={}, is_layout_dirty={}", commands_changed, *is_layout_dirty);

    if commands_changed || *is_layout_dirty {
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Rendering into target texture {}", target_texture);
        crate::module::graphics::render_ui(plugin, renderer, target_texture, width, height, sf, surface_format)?;

        *last_cmds = renderer.draw_commands.clone();
        *last_verts = renderer.vertices.clone();
        *is_layout_dirty = false;
    }

    *plugin.needs_redrawing.borrow_mut() = false;
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Caching UI");
    plugin.interface_cache.replace(ui.into_cache());

    // Store captured messages; dispatched right after render in render_plugin_if_needed
    if !captured_messages.is_empty() {
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Captured {} UI messages", captured_messages.len());
    }
    for msg in captured_messages {
        plugin.pending_messages.borrow_mut().push(PendingMessage {
            method: msg.method,
            value: msg.value,
        });
    }

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] END");
    Ok(())
}
