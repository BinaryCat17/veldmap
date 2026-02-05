use crate::{LocalConfig, LocalState};
use veldmap_gis_api::common::Empty;
use veldsdk::rpc::core::RpcResponse;

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    Ok(LocalState)
}

pub(crate) fn handle_empty(_state: &LocalState, _req: Empty) -> anyhow::Result<RpcResponse> {
    Ok(RpcResponse::default())
}
