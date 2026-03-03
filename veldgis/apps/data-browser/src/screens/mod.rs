//! screens/mod.rs — центральный реэкспорт всех экранов

pub mod search;
pub mod browse;
pub mod downloaded;
pub mod preview;

// Главные типы состояний
pub use search::SearchState;
pub use browse::BrowseState;
pub use downloaded::DownloadedState;
pub use preview::PreviewState;

// Сообщения с алиасами (чтобы не писать ::message::Message везде)
pub use search::message::Message as SearchMessage;
pub use browse::message::Message as BrowseMessage;
pub use downloaded::message::Message as DownloadedMessage;
pub use preview::message::Message as PreviewMessage;
