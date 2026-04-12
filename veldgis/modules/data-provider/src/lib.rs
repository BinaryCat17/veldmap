mod cdse;

use veldsdk::define_module;
use veldmap_api::dataprovider::{DownloadRequest, ListPathRequest, SearchRequest};
use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub(crate) struct LocalConfig {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub(crate) struct LocalState {
    pub identity: Identity,
}

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: cdse::module_init,
    handlers: {
        // Команды (входящие)
        "data-provider/download" => cdse::on_download,
        "data-provider/list_path" => cdse::on_list_path,
        "data-provider/search" => cdse::on_search,
    }
}
