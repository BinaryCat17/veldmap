//! preview/mod.rs — публичный интерфейс модуля предпросмотра изображения
//! (последний экран — ViewMode::View)

pub mod state;
pub mod message;
pub mod view;
pub mod update;

// Re-exports
pub use state::PreviewState;
pub use update::update;
pub use view::view;
