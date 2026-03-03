//! downloaded/mod.rs — публичный интерфейс модуля скачанных файлов
//! (аналогично search и browse)

pub mod state;
pub mod message;
pub mod view;
pub mod update;

// Re-exports
pub use state::DownloadedState;
pub use message::Message;
pub use update::update;
pub use view::view;
