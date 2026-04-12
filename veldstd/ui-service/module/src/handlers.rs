use veld_ui::proto::*;
use crate::state::{PluginUiState, LocalState};
use veldsdk::rpc::app as app_proto;
use crate::renderer::GpuRenderer;
use crate::converter;
use iced_core::{Point, Event, Size, Theme};
use iced_runtime::UserInterface;
use iced_graphics::Viewport;

pub fn handle_set_view(state: std::sync::Arc<std::sync::Mutex<LocalState>>, req: SetViewRequest) -> veldsdk::core::Command<()> {
    let mut state = state.lock().unwrap();
    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    
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
    veldsdk::core::Command::none()
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

pub fn handle_ui_event(state: std::sync::Arc<std::sync::Mutex<LocalState>>, req: HandleUiEventRequest) -> veldsdk::core::Command<()> {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] handle_ui_event START");
    let mut messages = Vec::new();
    if let Some(event_proto) = req.event {
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] Processing event for plugin: {}", req.plugin_id);
        let mut state_locked = state.lock().unwrap();
        if let Ok(mut msgs) = process_ui_event_recursive(&mut state_locked, &req.plugin_id, event_proto) {
            messages.append(&mut msgs);
        }
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] process_ui_event_recursive returned {} messages", messages.len());
    }
    for mut msg in messages {
        // Поддержка роутинга: если tag содержит '|', часть до '|' это топик, остальное - payload (value)
        let topic = if let Some(idx) = msg.message_tag.find('|') {
            let (t, payload) = msg.message_tag.split_at(idx);
            msg.value = payload[1..].to_string(); // пропускаем '|'
            t.to_string()
        } else {
            msg.message_tag.clone()
        };
        veldsdk::publish!(&topic, msg);
    }
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] handle_ui_event END");
    veldsdk::core::Command::none()
}

fn process_ui_event_recursive(state: &mut LocalState, plugin_id: &str, mut req_event: app_proto::UiEvent) -> anyhow::Result<Vec<UiEventResponse>> {
    let mut messages = Vec::new();

    let sub_events = std::mem::take(&mut req_event.sub_events);
    for sub_event in sub_events {
        let mut msgs = process_ui_event_recursive(state, plugin_id, sub_event)?;
        messages.append(&mut msgs);
    }

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
                {
                    *plugin.monitor_fps.borrow_mut() = f.monitor_fps;
                    *plugin.actual_fps.borrow_mut() = f.actual_fps;

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

                if !plugin.pending_events.borrow().is_empty() || *plugin.is_layout_dirty.borrow() {
                    veldsdk::vdebug!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] Frame: needs rendering, pending={}, dirty={}", 
                        plugin.pending_events.borrow().len(), *plugin.is_layout_dirty.borrow());
                    if let Some(_handle) = f.surface_handle {
                        veldsdk::vdebug!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] Calling render_plugin for {}", plugin_id);
                        let mut msgs = render_plugin(plugin, &mut state.renderer, plugin_id, state.surface_format)?;
                        veldsdk::vdebug!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] render_plugin returned {} messages", msgs.len());
                        messages.append(&mut msgs);
                    }
                } else {
                    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[MODULE-HANDLERS] Frame: no rendering needed");
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

    Ok(messages)
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

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str, surface_format: i32) -> anyhow::Result<Vec<UiEventResponse>> {
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] START for {}", plugin_id);
    let (width, height) = *plugin.canvas_size.borrow();
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] canvas size: {}x{}", width, height);
    if width == 0 || height == 0 { 
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Empty canvas, returning");
        return Ok(Vec::new()); 
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
    let _guard = crate::renderer::ScopeGuard::new(&mut renderer.font_system, &mut renderer.swash_cache);

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
        let texture_id = crate::graphics::render_ui(plugin, renderer, width, height, sf, surface_format)?;
        veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] display_frame with texture_id={}", texture_id);
        let _ = veldsdk::app::AppBridge::display_frame(texture_id);
        
        *last_cmds = renderer.draw_commands.clone();
        *last_verts = renderer.vertices.clone();
        *is_layout_dirty = false;
    }

    *plugin.needs_redrawing.borrow_mut() = false;
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] Caching UI");
    plugin.interface_cache.replace(ui.into_cache());
    
    let mut responses = Vec::new();
    for msg in captured_messages {
        responses.push(UiEventResponse {
            plugin_id: plugin_id.to_string(),
            message_tag: msg.tag,
            value: msg.value,
        });
    }
    
    veldsdk::vtrace!(veldsdk::FLAG_UI_HANDLERS, "[RENDER-PLUGIN] END, returning {} responses", responses.len());
    Ok(responses)
}
