mod provider;

use veldsdk::rpc::define_module;
use veldmap_gis_api::storage::{GetTileRequest, GetTileResponse};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {
    pub data_dir: String,
}

pub(crate) struct LocalState;

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: provider::module_init,
    handlers: {
        "get_tile" => provider::handle_get_tile : GetTileRequest => GetTileResponse,
    }
}
