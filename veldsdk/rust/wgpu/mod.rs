use crate::rpc::wgpu::{GpuResourceRequest, GpuResourceResponse, CreateShaderModule, CreateRenderPipeline};
use crate::rpc::core::ResourceHandle;
use crate::rpc::host::call_service;
use prost::Message;

pub mod wgpu_proxy;

pub fn create_shader(source: impl Into<String>, label: impl Into<String>) -> anyhow::Result<ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(crate::rpc::wgpu::gpu_resource_request::Command::CreateShader(CreateShaderModule {
            source: source.into(),
            label: label.into(),
        }))
    };
    let res_buf = call_service("wgpu", "create_resource", req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_buf[..])?;
    res.handle.ok_or_else(|| anyhow::anyhow!("Failed to create shader: {}", res.error))
}

pub fn create_pipeline(shader_id: u64, label: impl Into<String>, target_format: u32) -> anyhow::Result<ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(crate::rpc::wgpu::gpu_resource_request::Command::CreatePipeline(CreateRenderPipeline {
            shader_id,
            label: label.into(),
            vertex_entry: "vs_main".to_string(),
            fragment_entry: "fs_main".to_string(),
            vertex_layouts: Vec::new(),
            primitive_topology: 0, // TriangleList
            target_format,
            ..Default::default()
        }))
    };
    let res_buf = call_service("wgpu", "create_resource", req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_buf[..])?;
    res.handle.ok_or_else(|| anyhow::anyhow!("Failed to create pipeline: {}", res.error))
}

pub fn image_load_to_texture(path: impl Into<String>, usage: u32) -> anyhow::Result<ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(crate::rpc::wgpu::gpu_resource_request::Command::ImageLoadToTexture(crate::rpc::wgpu::ImageLoadToTexture {
            path: path.into(),
            usage,
            generate_mipmaps: false,
        }))
    };
    let res_buf = call_service("wgpu", "create_resource", req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_buf[..])?;
    res.handle.ok_or_else(|| anyhow::anyhow!("Failed to load image to texture: {}", res.error))
}

pub fn fs_read_to_buffer(path: impl Into<String>, usage: u32) -> anyhow::Result<ResourceHandle> {
    let req = GpuResourceRequest {
        command: Some(crate::rpc::wgpu::gpu_resource_request::Command::FsReadToBuffer(crate::rpc::wgpu::FsReadToBuffer {
            path: path.into(),
            usage,
        }))
    };
    let res_buf = call_service("wgpu", "create_resource", req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_buf[..])?;
    res.handle.ok_or_else(|| anyhow::anyhow!("Failed to read file to buffer: {}", res.error))
}
