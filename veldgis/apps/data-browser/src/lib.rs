//! Главный входной файл data-browser после полного рефакторинга
//! Только объявления модулей + вызов макроса (ничего лишнего)

mod state;
mod message;
mod handlers;
mod view;
mod common;
mod search;
mod browse;
mod downloaded;
mod preview;
mod service;

pub mod styles; // стили остаются публичными

// Публичные re-exports для удобства (чтобы не писать crate::state::AppState везде)
pub use state::AppState;
pub use message::AppMessage;
pub use veldmap_gis_api::dataprovider::DataProduct;
pub use common::BrowserItem;
pub use veldsdk::core::task::TaskStatus;

// Конфиг оставляем в common (как было раньше)
pub use common::LocalConfig;

use veld_ui::define_remote_ui_module;

define_remote_ui_module! {
    config: LocalConfig,
    state: AppState,
    message: AppMessage,
    init: handlers::module_init,
    view: view::view,
    handlers: {
        SwitchMode(mode) => handlers::handle_switch_mode;
        Search(m) => handlers::handle_search;
        Browse(m) => handlers::handle_browse;
        Downloaded(m) => handlers::handle_downloaded;
        Preview(m) => handlers::handle_preview;
        ClearError => handlers::handle_clear_error;
        CancelDownload => handlers::handle_cancel_download;
    }
}
