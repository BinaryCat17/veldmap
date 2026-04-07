pub mod wgpu_proxy;
pub use crate::rpc::compute::*;

crate::host_proxy! {
    service: "compute",
    create_resource: ComputeResourceRequest => ComputeResourceResponse,
}
