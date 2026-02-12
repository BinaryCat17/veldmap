pub mod common {
    include!(concat!(env!("OUT_DIR"), "/veldmap.common.rs"));
}
pub mod dataprovider {
    include!(concat!(env!("OUT_DIR"), "/veldmap.dataprovider.rs"));
}
pub mod storage {
    include!(concat!(env!("OUT_DIR"), "/veldmap.storage.rs"));
}
pub mod geomath {
    include!(concat!(env!("OUT_DIR"), "/veldmap.geomath.rs"));
}
pub mod render {
    include!(concat!(env!("OUT_DIR"), "/veldmap.render.rs"));
}
pub mod tileserver {
    include!(concat!(env!("OUT_DIR"), "/veldmap.tileserver.rs"));
}

// Генерируем типизированный прокси для провайдера данных
veldsdk::rpc_proxy! {
    service: "data-provider",
    @task search: dataprovider::SearchRequest => dataprovider::SearchResponse,
    @task download: dataprovider::DownloadRequest => dataprovider::DownloadResponse,
    @task list_path: dataprovider::ListPathRequest => dataprovider::ListPathResponse,
}
