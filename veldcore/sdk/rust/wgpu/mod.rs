pub mod wgpu_proxy;
pub use crate::rpc::wgpu::*;
use crate::prost::Message;

crate::host_proxy! {
    service: "wgpu",
    create_resource: GpuResourceRequest => GpuResourceResponse,
    submit: Submit => (),
}

pub fn create_shader(source: &str, label: &str) -> anyhow::Result<crate::rpc::core::ResourceHandle> {
    let req = GpuResourceRequest {
        instance_id: 0,
        command: Some(gpu_resource_request::Command::CreateShader(CreateShaderModule {
            source: source.to_string(),
            label: label.to_string(),
        }))
    };
    let res = raw::create_resource(&req)?;
    res.handle.ok_or_else(|| anyhow::anyhow!("WGPU Error: {}", res.error))
}

pub fn create_buffer(size: u64, usage: u32, readonly: bool) -> anyhow::Result<crate::rpc::core::ResourceHandle> {
    let req = GpuResourceRequest {
        instance_id: 0,
        command: Some(gpu_resource_request::Command::CreateBuffer(CreateBuffer {
            size, usage, mapped_at_creation: false, readonly
        }))
    };
    let res = raw::create_resource(&req)?;
    res.handle.ok_or_else(|| anyhow::anyhow!("WGPU Error: {}", res.error))
}

pub fn freeze_resource(id: u64) -> anyhow::Result<()> {
    let req = crate::rpc::core::FreezeResourceRequest { id };
    crate::rpc::host::call_service("system", "freeze_resource", req.encode_to_vec())?;
    Ok(())
}
