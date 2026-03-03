//! search/mod.rs — публичный интерфейс модуля поиска
//! (это точка входа для всего search-подмодуля)

pub mod state;
pub mod message;
pub mod view;
pub mod update;

// Re-exports для удобства использования из других модулей
pub use state::SearchState;
pub use update::update;
pub use view::view;
