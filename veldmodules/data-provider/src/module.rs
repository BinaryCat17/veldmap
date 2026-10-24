pub mod catalogue;
pub mod cdse;
pub mod imagery;
pub mod manifest;
pub mod mgrs;
pub mod s3;
pub mod scene;
pub mod time;

use aws_smithy_runtime_api::client::identity::Identity;
use crate::proto::data_provider::{DataProduct, SearchRequest};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

pub struct State {
    pub identity: Option<Identity>,
    pub pending_http: veldsdk::Correlator<Pending>,
    pub opening: veldsdk::Correlator<String>,
}

pub struct Pending {
    pub correlation_id: String,
    pub what: Asked,
}

pub enum Asked {
    List(String),
    Search { request: SearchRequest, widened: bool },
    Imagery { identifier: String, keys: Vec<String>, downloaded: bool, wanted: String },
    Manifest { identifier: String, keys: Vec<String>, downloaded: bool, wanted: String },
    Locate { name: String },
    Siblings { facts: scene::Facts, found: DataProduct },
}

pub use cdse::{
    module_init as hook_init,

    on_sign,
    on_list_path,
    on_search,
    on_open,
    on_imagery,
    on_locate,
    on_product_roots,

    on_http_result,
    on_open_result,
    on_resign,
};
