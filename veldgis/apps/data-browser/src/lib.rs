//! VeldMap Data Browser - GIS data discovery and download application

pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;

use veldsdk::define_module;
use veldmap_api::data_browser::{DownloadPressed, SearchRequest, BrowseRequest};
use veldmap_api::dataprovider::{DownloadStarted, DownloadProgress, Downloaded};

fn module_init(config: handlers::Config) -> anyhow::Result<state::State> {
    state::State::new(config)
}

define_module! {
    config: handlers::Config,
    state: state::State,
    init: module_init,
    handlers: {
        // Единая точка входа для всех UI событий
        "ui-service/event" => handlers::on_ui_event,
        
        // Подписки на события от data-provider (бизнес-логика)
        "data-provider/download_started" => handlers::download::on_download_started,
        "data-provider/download_progress" => handlers::download::on_download_progress,
        "data-provider/downloaded" => handlers::download::on_downloaded,
        
        "data-provider/search_result" => handlers::search::on_search_result,
        "data-provider/list_path_result" => handlers::browse::on_list_result,
    }
}
