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
/// Вызывается системой при получении сообщения на топик "ui-service/event".
pub fn on_ui_event(state: Arc<Mutex<crate::state::State>>, event: UiEventResponse) -> anyhow::Result<()> {
    // 1. Проверяем, что событие адресовано нам
    if event.plugin_id != "data-browser" {
        return Ok(());
    }

    // 2. Используем механизм из SDK для пересылки события на конкретный топик
    // Это спровоцирует вызов одного из наших хендлеров (nav_browse, search и т.д.) через RPC шину
    veld_ui::dispatch_event(event)?;

    // 3. В конце делаем финальный рендер
    let mut guard = state.lock().unwrap();
    veld_ui::app::render("data-browser", &mut guard);

    Ok(())
}
