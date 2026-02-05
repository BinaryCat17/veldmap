pub mod wgpu_proxy;
pub use crate::rpc::wgpu::*;

crate::rpc_proxy! {
    service: "wgpu",
    create_resource: GpuResourceRequest => GpuResourceResponse,
    submit: Submit => (),
}

pub fn create_shader(source: &str, label: &str) -> anyhow::Result<crate::rpc::core::ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(gpu_resource_request::Command::CreateShader(CreateShaderModule {
            source: source.to_string(),
            label: label.to_string(),
        }))
    };
    let res = raw::create_resource(&req)?;
    res.handle.ok_or_else(|| anyhow::anyhow!("WGPU Error: {}", res.error))
}

pub fn create_buffer(size: u64, usage: u32, _label: &str) -> anyhow::Result<crate::rpc::core::ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
            size, usage, mapped_at_creation: false,
        }))
    };
    let res = raw::create_resource(&req)?;
    res.handle.ok_or_else(|| anyhow::anyhow!("WGPU Error: {}", res.error))
}
