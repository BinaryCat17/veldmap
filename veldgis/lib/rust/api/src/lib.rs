pub mod common {
    include!(concat!(env!("OUT_DIR"), "/veldmap.common.rs"));
}
pub mod dataprovider {
    include!(concat!(env!("OUT_DIR"), "/veldmap.dataprovider.rs"));
}

// Генерируем типизированный прокси для провайдера данных
veldsdk::rpc_proxy! {
    service: "data-provider",
    search: dataprovider::SearchRequest => dataprovider::SearchResponse,
    download: dataprovider::DownloadRequest => dataprovider::DownloadResponse,
    list_path: dataprovider::ListPathRequest => dataprovider::ListPathResponse,
}
