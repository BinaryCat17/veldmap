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
    let canvas_size = state.canvas_size;

    // Get or create plugin. Seed its canvas size from the shared window size so
    // the very first render doesn't use PluginUiState::new()'s placeholder size.
    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(|| {
        let p = PluginUiState::new();
        if canvas_size.0 > 0 && canvas_size.1 > 0 {
            *p.canvas_size.borrow_mut() = canvas_size;
        }
        p
    });
    
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
    let Some(plugin) = state.plugins.get(plugin_id) else { return };
    let needs_render = *plugin.needs_redrawing.borrow() || *plugin.is_layout_dirty.borrow();
    let surface_handle = *plugin.surface_handle.borrow();

    if let Some(handle) = surface_handle {
        if needs_render {
            // Get plugin and renderer separately to avoid borrow issues
            let plugin_ref = state.plugins.get_mut(plugin_id).unwrap() as *mut PluginUiState;
            let renderer_ref = &mut state.renderer as *mut GpuRenderer;
            unsafe {
                if let Err(e) = render_plugin(&mut *plugin_ref, &mut *renderer_ref, plugin_id, surface_format, handle) {
                    veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[render_plugin_if_needed] render_plugin failed: {}", e);
                }
            }
        }
    }

    // Dispatch messages captured by iced during this render (button presses etc.)
    // immediately: deferring them to the next set_view would deadlock, because the
    // plugin only sends set_view after reacting to these very messages.
    let plugin = state.plugins.get(plugin_id).unwrap();
    let pending = plugin.pending_messages.borrow_mut().drain(..).collect::<Vec<_>>();
    for msg in pending {
        dispatch_event(UiEventResponse {
            plugin_id: plugin_id.to_string(),
            method: msg.method,
            value: msg.value,
        });
    }
}

pub fn handle_ui_event(state: &mut State, event_proto: app_proto::UiEvent) {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] handle_ui_event START (via publish)");

    // Track the shared window size independently of plugin registration: a plugin
    // only gets added to `state.plugins` via `set_view`, which a plugin only sends
    // in response to a `frame` tick - so `frame` must not depend on a plugin
    // already being registered, or nothing would ever bootstrap.
    if let Some(app_proto::ui_event::Event::Resize(r)) = &event_proto.event {
        state.canvas_size = (r.width, r.height);
    }

    let is_frame = matches!(event_proto.event, Some(app_proto::ui_event::Event::Frame(_)));

    // Process event for ALL plugins - we don't know which one it's for
    // ui-service stores layouts of all plugins.
    let plugin_ids: Vec<String> = state.plugins.keys().cloned().collect();

    for plugin_id in plugin_ids {
        if let Err(e) = process_ui_event(state, &plugin_id, event_proto.clone()) {
            veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] process_ui_event failed for {}: {}", plugin_id, e);
        }
    }

    // On each frame tick render plugins with pending changes: input-driven redraws
    // and animation (scroll inertia) are the renderer's own responsibility.
    if is_frame {
        let surface_format = state.surface_format;
        let plugin_ids: Vec<String> = state.plugins.keys().cloned().collect();
        for plugin_id in plugin_ids {
            render_plugin_if_needed(state, &plugin_id, surface_format);
        }
    }

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] handle_ui_event END");
}

fn process_ui_event(state: &mut State, plugin_id: &str, req_event: app_proto::UiEvent) -> anyhow::Result<()> {
    let plugin = state.plugins.entry(plugin_id.to_string()).or_insert_with(PluginUiState::new);

    if let Some(ev) = req_event.event {
        match ev {
            app_proto::ui_event::Event::Resize(r) => {
                *plugin.canvas_size.borrow_mut() = (r.width, r.height);
                *plugin.scale_factor.borrow_mut() = r.scale_factor;
                *plugin.needs_redrawing.borrow_mut() = true;
            }
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
                // Save surface handle for rendering (extract id from ResourceHandle)
                if let Some(handle) = f.surface_handle {
                    *plugin.surface_handle.borrow_mut() = Some(handle.id);
                }
                
                *plugin.monitor_fps.borrow_mut() = f.monitor_fps;
                *plugin.actual_fps.borrow_mut() = f.actual_fps;

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

                // Process events with iced to capture messages
                if !plugin.pending_events.borrow().is_empty() || *plugin.is_layout_dirty.borrow() {
                    if let Some(handle) = *plugin.surface_handle.borrow() {
                        let _ = process_iced_events(plugin, plugin_id, handle)?;
                    }
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

/// Mark the plugin for redraw when there is pending input.
/// The events themselves stay in `pending_events`: render_plugin drains them and
/// feeds them to iced's ui.update(), which is what actually produces messages.
fn process_iced_events(plugin: &PluginUiState, plugin_id: &str, _surface_handle: u64) -> anyhow::Result<()> {
    if plugin.pending_events.borrow().is_empty() && !*plugin.is_layout_dirty.borrow() {
        return Ok(());
    }

    *plugin.needs_redrawing.borrow_mut() = true;

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[PROCESS-ICED] Events queued for {}", plugin_id);
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

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, surface_format: i32, _surface_handle: u64) -> anyhow::Result<()> {
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
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Calling render_ui");
        let texture_id = crate::module::graphics::render_ui(plugin, renderer, width, height, sf, surface_format)?;
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] display_frame with texture_id={}", texture_id);

        let cmd = veldsdk::rpc::app::AppDisplayCommand {
            command: Some(veldsdk::rpc::app::app_display_command::Command::DrawFrame(veldsdk::rpc::app::DrawFrame { texture_id }))
        };
        crate::calls::app::display(&cmd);

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
