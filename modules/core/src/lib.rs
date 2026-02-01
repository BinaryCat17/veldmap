mod node;

use veldmap_rust_rpc::define_module;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub(crate) struct LocalConfig {}

pub(crate) struct LocalState {
    pub(crate) config: LocalConfig,
}

define_module! {
    config: LocalConfig,
    state: LocalState,
    init: node::module_init,
    custom_handler: node::custom_handle_rpc
}
