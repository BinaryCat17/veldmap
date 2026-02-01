mod server;

use veldsdk::define_module;
use veldmap_gis_api::tileserver::{TileRequest, TileResponse};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct LocalConfig {
    #[allow(dead_code)]
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
