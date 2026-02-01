mod server;

use veldsdk::rpc::define_module;
use veldsdk::rpc::tile_server::{TileRequest, TileResponse};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {
    pub port: u16,
}

pub(crate) struct LocalState;

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: server::module_init,
    handlers: {
        "handle_request" => server::handle_tile_request : TileRequest => TileResponse,
    }
}
