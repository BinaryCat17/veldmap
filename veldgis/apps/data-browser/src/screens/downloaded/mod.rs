//! downloaded/mod.rs — публичный интерфейс модуля скачанных файлов
//! (аналогично search и browse)

pub mod state;
pub mod message;
pub mod view;
pub mod update;

// Re-exports
pub use state::DownloadedState;
pub use update::{update, update_download_file};
pub use view::view;
