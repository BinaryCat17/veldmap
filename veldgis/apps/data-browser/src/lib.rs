//! Главный lib.rs после финальной реорганизации

mod app;
mod screens;

pub mod styles;
pub mod common;
pub mod service;

pub use app::{AppState, AppMessage};
pub use common::{BrowserItem, LocalConfig};
pub use veldsdk::core::task::TaskStatus;
pub use veldmap_api::dataprovider::DataProduct;

use veld_ui::define_remote_ui_module;

define_remote_ui_module! {
    config: LocalConfig,
    state: AppState,
    message: AppMessage,
    init: app::handlers::module_init,
    view: app::view::view,
    handlers: {
        SwitchMode(mode) => app::handlers::handle_switch_mode;
        Search(m) => app::handlers::handle_search;
        Browse(m) => app::handlers::handle_browse;
        Downloaded(m) => app::handlers::handle_downloaded;
        Preview(m) => app::handlers::handle_preview;
        ClearError => app::handlers::handle_clear_error;
        CancelDownload => app::handlers::handle_cancel_download;
    }
}
