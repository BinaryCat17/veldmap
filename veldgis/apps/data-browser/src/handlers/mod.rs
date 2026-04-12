//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod download;
pub mod nav;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_screen: Option<String>,
}

use std::sync::{Arc, Mutex};
use veld_ui::proto::UiEventResponse;

/// Единая точка входа для всех UI событий.
pub fn on_ui_event(state: &mut crate::state::State, event: UiEventResponse) {
    if event.plugin_id != "data-browser" {
        return;
    }

    // 1. Диспетчеризация через шину (как и было задумано в новой архитектуре)
    // Это вызовет соответствующий хэндлер из define_module!
    let _ = veld_ui::dispatch_event(event);

    // 2. Финальный рендер
    let root = crate::view::build_root(state);
    
    let (w, h) = state.last_layout.as_ref()
        .map(|l| (l.width, l.height))
        .unwrap_or((1024, 768));

    veld_ui::app::render(
        "data-browser", 
        root, 
        &mut state.last_layout,
        w, h
    );
}
