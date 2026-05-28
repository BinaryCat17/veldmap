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
    
    // Get or create plugin
    let plugin = state.plugins.entry(plugin_id.clone()).or_insert_with(PluginUiState::new);
    
    // 1. Dispatch pending messages for this plugin BEFORE rendering
    // This ensures the plugin has processed all UI events before we render
    let pending = plugin.pending_messages.borrow_mut().drain(..).collect::<Vec<_>>();
    for msg in pending {
        let resp = UiEventResponse {
            plugin_id: plugin_id.clone(),
            message_tag: msg.message_tag,
            value: msg.value,
        };
        let _ = dispatch_event(resp);
    }
    
    // 2. Update layout
    match req.update {
        Some(set_view_request::Update::FullLayout(l)) => {
            plugin.layout = l;
            *plugin.is_layout_dirty.borrow_mut() = true;
            *plugin.needs_redrawing.borrow_mut() = true;
        }
        Some(set_view_request::Update::Patch(patch)) => {
            if let Some(ref mut root) = plugin.layout.root {
                for update in patch.updates {
                    if let Some(new_widget) = update.new_widget {
                        apply_widget_update(root, update.widget_id, new_widget);
                    }
                }
                plugin.layout.width = patch.width;
                plugin.layout.height = patch.height;
                plugin.layout.hash = patch.new_hash;
                *plugin.is_layout_dirty.borrow_mut() = true;
                *plugin.needs_redrawing.borrow_mut() = true;
            }
        }
        None => {}
    }
    
    // 3. Check if we need to render
    let needs_render = *plugin.needs_redrawing.borrow() || *plugin.is_layout_dirty.borrow();
    let surface_handle = *plugin.surface_handle.borrow();
    
    // plugin borrow ends here before rendering
    let _ = plugin;
    
    // 4. Render via iced if we have surface handle and need to render
    if let Some(handle) = surface_handle {
        if needs_render {
            // Get plugin and renderer separately to avoid borrow issues
            let plugin_ref = state.plugins.get_mut(&plugin_id).unwrap() as *mut PluginUiState;
            let renderer_ref = &mut state.renderer as *mut GpuRenderer;
            unsafe {
                if let Err(e) = render_plugin(&mut *plugin_ref, &mut *renderer_ref, &plugin_id, surface_format, handle) {
                    veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[handle_set_view] render_plugin failed: {}", e);
                }
            }
        }
    }
    
}

fn apply_widget_update(current: &mut Widget, id: u64, new_w: Widget) -> bool {
    if current.id == id {
        *current = new_w;
        return true;
    }

    match &mut current.r#type {
        Some(widget::Type::Column(c)) => { for child in &mut c.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Row(r)) => { for child in &mut r.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Stack(s)) => { for child in &mut s.children { if apply_widget_update(child, id, new_w.clone()) { return true; } } }
        Some(widget::Type::Container(c)) => { if let Some(child) = &mut c.child { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Scrollable(s)) => { if let Some(child) = &mut s.content { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Tooltip(t)) => { if let Some(child) = &mut t.content { return apply_widget_update(child, id, new_w); } }
        Some(widget::Type::Button(b)) => { if let Some(child) = &mut b.child { return apply_widget_update(child, id, new_w); } }
        _ => {}
    }
    false
}

pub fn handle_ui_event(state: &mut State, event_proto: app_proto::UiEvent) {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] handle_ui_event START (via publish)");
    
    let is_frame = matches!(event_proto.event, Some(app_proto::ui_event::Event::Frame(_)));
    
    // Process event for ALL plugins - we don't know which one it's for
    // ui-service stores layouts of all plugins.
    let plugin_ids: Vec<String> = state.plugins.keys().cloned().collect();
    
    for plugin_id in plugin_ids {
        if let Err(e) = process_ui_event(state, &plugin_id, event_proto.clone()) {
            veldsdk::verror!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] process_ui_event failed for {}: {}", plugin_id, e);
        }
    }
    
    // Publish frame event to all subscribers after processing all plugins
    if is_frame {
        let mut frame_event = crate::proto::ui::FrameEvent { width: 0, height: 0, dt: 0.0 };
        if let Some(app_proto::ui_event::Event::Frame(f)) = event_proto.event {
            frame_event.dt = f.dt;
        }
        // Use dimensions from the first plugin with a valid canvas size
        for plugin in state.plugins.values() {
            let (w, h) = *plugin.canvas_size.borrow();
            if w > 0 && h > 0 {
                frame_event.width = w;
                frame_event.height = h;
                break;
            }
        }
        if frame_event.width > 0 && frame_event.height > 0 {
            veldsdk::publish!("ui-service/frame", frame_event);
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

/// Process iced events and capture messages to pending_messages
fn process_iced_events(plugin: &PluginUiState, plugin_id: &str, _surface_handle: u64) -> anyhow::Result<()> {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[PROCESS-ICED] START for {}", plugin_id);
    
    let events = std::mem::take(&mut *plugin.pending_events.borrow_mut());
    if events.is_empty() && !*plugin.is_layout_dirty.borrow() {
        return Ok(());
    }
    
    // For now, we don't have access to renderer here - just store that we need processing
    // The actual rendering happens in handle_set_view when plugin calls render()
    *plugin.needs_redrawing.borrow_mut() = true;
    
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[PROCESS-ICED] Events queued for {}", plugin_id);
    Ok(())
}

/// Внутренняя функция для диспетчеризации событий плагину.
/// Вызывается в handle_set_view перед рендерингом.
fn dispatch_event(event: UiEventResponse) -> anyhow::Result<()> {
    let topic = event.message_tag.clone();
    veldsdk::output!(&topic, event);
    Ok(())
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
        let _ = veldsdk::publish!("app/display", cmd);

        *last_cmds = renderer.draw_commands.clone();
        *last_verts = renderer.vertices.clone();
        *is_layout_dirty = false;
    }

    *plugin.needs_redrawing.borrow_mut() = false;
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Caching UI");
    plugin.interface_cache.replace(ui.into_cache());

    // Store captured messages for dispatch on next set_view
    for msg in captured_messages {
        plugin.pending_messages.borrow_mut().push(PendingMessage {
            message_tag: msg.tag,
            value: msg.value,
        });
    }

    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] END");
    Ok(())
}
