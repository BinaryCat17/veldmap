pub mod common {
    include!(concat!(env!("OUT_DIR"), "/veldmap.common.rs"));
}
pub mod dataprovider {
    include!(concat!(env!("OUT_DIR"), "/veldmap.dataprovider.rs"));
}
pub mod data_browser {
    include!(concat!(env!("OUT_DIR"), "/veldmap.data_browser.rs"));
}
pub mod ui {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
}

// Псевдонимы для совместимости с prost-generated кода
pub mod core {
    pub use veldsdk::rpc::core::*;
}
pub mod app {
    pub use veldsdk::rpc::app::*;
}

// Генерируем транспорт для провайдера данных
// (Удалено: rpc_proxy больше не используется в FaF архитектуре)

// Генерируем транспорт для data-browser
// (Удалено: rpc_proxy больше не используется в FaF архитектуре)

// Генерируем транспорт для ui-service
// (Удалено: rpc_proxy больше не используется в FaF архитектуре)
