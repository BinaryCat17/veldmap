mod handlers;

use veldmap_rust_rpc::define_module;
use veldmap_rust_rpc::services::RpcResponse;
use veldmap_rust_rpc::common::Empty;
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
