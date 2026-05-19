pub mod cdse;

use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub struct LocalState {
    pub identity: Identity,
    pub pending_downloads: std::collections::HashSet<String>,
    pub pending_http: std::collections::HashMap<String, String>,
}

// Re-export handlers to match the expected names in generated code
pub use cdse::{
    module_init as init,
    on_download,
    on_list_path,
    on_search,
    on_fs_download_result,
    on_http_result,
};