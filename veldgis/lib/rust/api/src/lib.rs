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
veldsdk::rpc_proxy! {
    service: "data-provider",
    namespace: data_provider,
    search: dataprovider::SearchRequest => dataprovider::SearchResponse,
    download: dataprovider::DownloadRequest => dataprovider::DownloadResponse,
    list_path: dataprovider::ListPathRequest => dataprovider::ListPathResponse,
}

// Генерируем транспорт для data-browser (используем _client суффикс чтобы избежать конфликта)
veldsdk::rpc_proxy! {
    service: "data-browser",
    namespace: data_browser_client,
    handle_ui_event: data_browser::HandleUiEventRequest => data_browser::HandleUiEventResponse,
    navigate: data_browser::NavigateRequest => data_browser::NavigateResponse,
    search: data_browser::SearchRequest => data_browser::SearchResponse,
    browse: data_browser::BrowseRequest => data_browser::BrowseResponse,
    download: data_browser::DownloadRequest => data_browser::DownloadResponse,
    render: data_browser::RenderRequest => data_browser::RenderResponse,
}

// Генерируем транспорт для ui-service
veldsdk::rpc_proxy! {
    service: "ui-service",
    namespace: ui_service,
    set_view: ui::SetViewRequest => ui::SetViewResponse,
    handle_ui_event: ui::HandleUiEventRequest => ui::HandleUiEventResponse,
}
