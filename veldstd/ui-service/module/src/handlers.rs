use veld_ui::proto::*;
use crate::state::{PluginUiState, LocalState};
use veldsdk::rpc::app as app_proto;
use crate::renderer::GpuRenderer;
use crate::converter;
use iced_core::{Point, Event, Size};
use iced_runtime::user_interface::UserInterface;
use iced_graphics::Viewport;
use prost::Message;
use veldsdk::rpc::host::call_service;

pub fn handle_set_view(state: &mut LocalState, req: SetViewRequest) -> anyhow::Result<SetViewResponse> {
    let plugin = state.plugins.entry(req.plugin_id.clone()).or_insert_with(PluginUiState::new);
    if let Some(l) = req.layout {
        plugin.layout = l;
        *plugin.needs_redrawing.borrow_mut() = true;
    }
    Ok(SetViewResponse {})
}

pub fn handle_ui_event(state: &mut LocalState, req: HandleUiEventRequest) -> anyhow::Result<HandleUiEventResponse> {
    if let Some(plugin) = state.plugins.get_mut(&req.plugin_id) {
        if let Some(event_proto) = req.event {
            if let Some(ev) = event_proto.event {
                match ev {
                    app_proto::ui_event::Event::Resize(r) => {
                        *plugin.canvas_size.borrow_mut() = (r.width, r.height);
                        *plugin.scale_factor.borrow_mut() = r.scale_factor;
                        *plugin.needs_redrawing.borrow_mut() = true;
                    }
                    app_proto::ui_event::Event::CursorMoved(c) => {
                        let sf = *plugin.scale_factor.borrow();
                        let pos = Point::new(c.x / sf, c.y / sf);
                        *plugin.cursor_position.borrow_mut() = pos;
                        plugin.pending_events.borrow_mut().push(Event::Mouse(iced_core::mouse::Event::CursorMoved { position: pos }));
                    }
                    app_proto::ui_event::Event::Click(c) => {
                        let sf = *plugin.scale_factor.borrow();
                        let pos = Point::new(c.x / sf, c.y / sf);
                        *plugin.cursor_position.borrow_mut() = pos;
                        
                        let button = match c.button {
                            1 => iced_core::mouse::Button::Left,
                            2 => iced_core::mouse::Button::Right,
                            3 => iced_core::mouse::Button::Middle,
                            _ => iced_core::mouse::Button::Left,
                        };
                        let mut events = plugin.pending_events.borrow_mut();
                        // Важно: сначала обновляем позицию курсора, чтобы iced знал где произошел клик
                        events.push(Event::Mouse(iced_core::mouse::Event::CursorMoved { position: pos }));
                        
                        if c.pressed {
                            events.push(Event::Mouse(iced_core::mouse::Event::ButtonPressed(button)));
                        } else {
                            events.push(Event::Mouse(iced_core::mouse::Event::ButtonReleased(button)));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(HandleUiEventResponse {})
}

pub fn handle_render(state: &mut LocalState, req: RenderRequest) -> anyhow::Result<RenderResponse> {
    let renderer = &mut state.renderer;
    if let Some(plugin) = state.plugins.get_mut(&req.plugin_id) {
        render_plugin(plugin, renderer, &req.plugin_id)?;
    }
    Ok(RenderResponse {})
}

fn render_plugin(plugin: &PluginUiState, renderer: &mut GpuRenderer, plugin_id: &str) -> anyhow::Result<()> {
    let (width, height) = *plugin.canvas_size.borrow();
    if width == 0 || height == 0 { return Ok(()); }
    
    let sf = *plugin.scale_factor.borrow();
    let cursor_pos = *plugin.cursor_position.borrow();
    let cursor = iced_core::mouse::Cursor::Available(cursor_pos);
    let events = std::mem::take(&mut *plugin.pending_events.borrow_mut());
    
    let viewport = Viewport::with_physical_size(Size::new(width, height), sf.into());
    let mut captured_messages = Vec::new();
    
    renderer.clear();
    let element = converter::convert_layout(&plugin.layout);
    
    let cache = plugin.interface_cache.replace(iced_runtime::user_interface::Cache::default());
    let mut ui = UserInterface::build(
        element,
        viewport.logical_size(),
        cache,
        renderer,
    );
    
    let mut clipboard = iced_core::clipboard::Null;
    let (ui_state, _) = ui.update(&events, cursor, renderer, &mut clipboard, &mut captured_messages);
    
    let mut needs_redrawing = plugin.needs_redrawing.borrow_mut();
    let should_draw = *needs_redrawing || !events.is_empty() || matches!(ui_state, iced_runtime::user_interface::State::Outdated);
    
    if should_draw {
        renderer.render_to_texture(plugin, &mut ui, width, height, sf, cursor)?;
        *needs_redrawing = false;
    }
    
    plugin.interface_cache.replace(ui.into_cache());
    
    for msg in captured_messages {
        let event_res = UiEventResponse {
            plugin_id: plugin_id.to_string(),
            message_tag: msg.tag,
            value: msg.value,
        };
        let _ = call_service(plugin_id, "handle_ui_message", event_res.encode_to_vec());
    }
    
    Ok(())
}