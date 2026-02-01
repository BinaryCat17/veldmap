mod handlers;

use veldsdk::define_module;
use veldsdk::rpc::services::RpcResponse;
use veldmap_gis_api::common::Empty;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalState;

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: handlers::module_init,
    handlers: {
        "noop" => handlers::handle_empty : Empty => RpcResponse,
    }
}
