//! VeldMap Data Browser - GIS data discovery and download application

pub mod handlers;
pub mod state;
pub mod view;
pub mod components;
pub mod styles;

use veldsdk::define_module;

fn module_init(config: handlers::Config) -> anyhow::Result<state::State> {
    state::State::new(config)
}

define_module! {
    config: handlers::Config,
    state: state::State,
    init: module_init,
    handlers: {
        // Frame signal - рендер каждый кадр (единственное место с render!)
        "ui-service/frame" => handlers::on_frame,
        
        // UI события (диспатчатся из ui-service при обработке пользовательского ввода)
        "data-browser/nav_browse" => handlers::nav::on_nav_browse,
        "data-browser/nav_search" => handlers::nav::on_nav_search,
        "data-browser/nav_downloaded" => handlers::nav::on_nav_downloaded,
        
        "data-browser/browse" => handlers::browse::on_browse,
        "data-browser/browse_up" => handlers::browse::on_browse_up,
        
        "data-browser/search" => handlers::search::on_search,
        "data-browser/search_input" => handlers::search::on_search_input,
        
        "data-browser/download_pressed" => handlers::download::on_download_pressed,
        "data-browser/view_pressed" => handlers::download::on_view_pressed,
        
        // События данных (бизнес-логика) - только меняют state, НЕ рендерят
        "data-provider/download_started" => handlers::download::on_download_started,
        "data-provider/downloaded" => handlers::download::on_downloaded,
        
        "data-provider/search_result" => handlers::search::on_search_result,
        "data-provider/list_path_result" => handlers::browse::on_list_result,
        "fs/list_result" => handlers::nav::on_fs_list_result,
        
        // Async callbacks from host services
        "image/load_result" => handlers::download::on_image_loaded,
    }
}
