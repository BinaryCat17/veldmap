//! VeldMap Data Browser - GIS data discovery and download application

pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;
pub mod common;

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
        // UI события от пользователя
        "data-browser/download_pressed" => handlers::download::on_download_pressed : DownloadPressed,
        "data-browser/search" => handlers::search::on_search : SearchRequest,
        "data-browser/browse" => handlers::browse::on_browse : BrowseRequest,
        
        // Подписки на события от data-provider
        "data-provider/download_started" => handlers::download::on_download_started : DownloadStarted,
        "data-provider/download_progress" => handlers::download::on_download_progress : DownloadProgress,
        "data-provider/downloaded" => handlers::download::on_downloaded : Downloaded,
    }
}
