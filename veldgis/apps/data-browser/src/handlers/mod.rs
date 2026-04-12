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
/// Здесь происходит диспетчеризация и финальный рендер.
pub fn on_ui_event(state: Arc<Mutex<crate::state::State>>, event: UiEventResponse) -> anyhow::Result<()> {
    // Проверяем, что событие адресовано нам
    if event.plugin_id != "data-browser" {
        return Ok(());
    }

    let mut guard = state.lock().unwrap();
    let mut handled = true;

    match event.message_tag.as_str() {
        "data-browser/nav_browse" => nav::on_nav_browse(&mut guard, event.value)?,
        "data-browser/nav_search" => nav::on_nav_search(&mut guard, event.value)?,
        "data-browser/nav_downloaded" => nav::on_nav_downloaded(&mut guard, event.value)?,
        
        "data-browser/browse" => browse::on_browse(&mut guard, event.value)?,
        "data-browser/browse_up" => browse::on_browse_up(&mut guard, event.value)?,
        
        "data-browser/search" => search::on_search(&mut guard, event.value)?,
        "data-browser/search_input" => search::on_search_input(&mut guard, event.value)?,
        
        "data-browser/download_pressed" => download::on_download_pressed(&mut guard, event.value)?,
        _ => {
            handled = false;
        }
    }

    // Если мы обработали событие, обновляем View
    if handled {
        veld_ui::app::render("data-browser", &mut guard);
    }

    Ok(())
}
