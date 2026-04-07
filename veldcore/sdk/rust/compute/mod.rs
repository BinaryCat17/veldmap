pub mod wgpu_proxy;
pub use crate::rpc::compute::*;
use crate::prost::Message;

crate::host_proxy! {
    service: "compute",
    create_resource: ComputeResourceRequest => ComputeResourceResponse,
}
