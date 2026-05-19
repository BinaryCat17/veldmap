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
