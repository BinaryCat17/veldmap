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

pub fn on_ui_event(state: Arc<Mutex<crate::state::State>>, event: UiEventResponse) -> anyhow::Result<()> {
    veld_ui::handle_ui_event!("data-browser", state, event, {
        "data-browser/nav_browse" => nav::on_nav_browse,
        "data-browser/nav_search" => nav::on_nav_search,
        "data-browser/nav_downloaded" => nav::on_nav_downloaded,
        
        "data-browser/browse" => browse::on_browse,
        "data-browser/browse_up" => browse::on_browse_up,
        
        "data-browser/search" => search::on_search,
        "data-browser/search_input" => search::on_search_input,
        
        "data-browser/download_pressed" => download::on_download_pressed,
    })
}
