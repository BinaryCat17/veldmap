//! VeldMap Data Browser - GIS data discovery and download application

pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;
pub mod common;

use veldsdk::define_module;

fn module_init(config: handlers::Config) -> anyhow::Result<state::State> {
    state::State::new(config)
}

define_module! {
    config: handlers::Config,
    state: state::State,
    init: module_init,
    handlers: {
        // UI события от хоста
        "handle_ui_event" => handlers::handle_ui_event : veldmap_api::data_browser::HandleUiEventRequest => veldmap_api::data_browser::HandleUiEventResponse,
        
        // Поиск
        "search" => handlers::search::search : veldmap_api::data_browser::SearchRequest => veldmap_api::data_browser::SearchResponse,
        
        // Браузинг
        "browse" => handlers::browse::browse : veldmap_api::data_browser::BrowseRequest => veldmap_api::data_browser::BrowseResponse,
        
        // Загрузка
        "download" => handlers::download::download : veldmap_api::data_browser::DownloadRequest => veldmap_api::data_browser::DownloadResponse,
    }
}
