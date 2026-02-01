mod cdse;

use veldsdk::define_module;
use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse};
use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub(crate) struct LocalConfig {
    pub access_key: String,
    pub secret_key: String,
}

pub(crate) struct LocalState {
    pub config: LocalConfig,
    pub identity: Identity,
}

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: cdse::module_init,
    handlers: {
        "search" => cdse::search : SearchRequest => SearchResponse,
        "download" => cdse::download : DownloadRequest => DownloadResponse,
        "list_path" => cdse::list_path : ListPathRequest => ListPathResponse,
    }
}
