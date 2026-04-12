use std::sync::{Arc, Mutex};
use veldsdk::core::Command;
use veldmap_api::app::UiEvent;
use veldmap_api::ui::HandleUiEventRequest;

pub fn on_ui_event(state: Arc<Mutex<crate::state::State>>, event: UiEvent) -> Command<()> {
    let mut needs_render = false;
    {
        let mut guard = state.lock().unwrap();
        if !guard.global.has_rendered {
            guard.global.has_rendered = true;
            needs_render = true;
        }
    }
    
    if needs_render {
        crate::view::render(&state.lock().unwrap());
    }

    veldsdk::publish!("ui-service/handle_ui_event", HandleUiEventRequest {
        plugin_id: "data-browser".to_string(),
        event: Some(event),
    });
    Command::none()
}