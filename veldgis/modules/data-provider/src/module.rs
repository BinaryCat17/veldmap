pub mod cdse;

use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub struct State {
    pub identity: Identity,
    pub pending_downloads: std::collections::HashSet<String>,
    pub pending_http: std::collections::HashMap<String, String>,
}

// Re-export handlers to match the expected names in generated code
pub use cdse::{
    module_init,
    
    // calls
    on_input_download,
    on_input_list_path,
    on_input_search,

    // subs
    on_sub_fs_download_result,
    on_sub_http_result,
};