use std::sync::{Arc, Mutex};
use veldsdk::core::Command;
use veldmap_api::app::UiEvent;
use veldmap_api::ui::HandleUiEventRequest;

pub fn on_ui_event(_state: Arc<Mutex<crate::state::State>>, event: UiEvent) -> Command<()> {
    veldsdk::publish!("ui-service/handle_ui_event", HandleUiEventRequest {
        plugin_id: "data-browser".to_string(),
        event: Some(event),
    });
    Command::none()
}