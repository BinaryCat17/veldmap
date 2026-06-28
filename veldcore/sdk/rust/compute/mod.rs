pub mod wgpu_proxy;
pub use crate::rpc::compute::*;

pub fn create_resource(req: &ComputeResourceRequest) -> anyhow::Result<ComputeResourceResponse> {
    use prost::Message;
    let payload = req.encode_to_vec();
    let res_bytes = crate::rpc::host::graphics_create_resource(payload)?;
    Ok(ComputeResourceResponse::decode(&res_bytes[..])?)
}
