mod cdse;

use veldsdk::define_module;
use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub(crate) struct LocalConfig {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub(crate) struct LocalState {
    pub identity: Identity,
    pub pending_downloads: std::collections::HashSet<String>,
    pub pending_http: std::collections::HashMap<String, String>,
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
        
        // Async callbacks from host services
        "network/fs_download_result" => cdse::on_fs_download_result,
        "network/http_result" => cdse::on_http_result,
    }
}
