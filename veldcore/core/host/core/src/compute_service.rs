use crate::resources::{ResourceManager, Resource, GpuObject};
use crate::core::ResourceHandle;
use crate::compute::{
    ComputeResourceRequest, ComputeResourceResponse, Submit, CommandBuffer,
    compute_resource_request::Command as ComputeCommand,
    wgpu_command::Command as WgpuCommand,
    bind_group_layout_entry::Ty,
};
use prost::Message;
use std::sync::{Arc, Mutex};

pub static PENDING_OPS: Mutex<Vec<PendingRenderOp>> = Mutex::new(Vec::new());

pub struct PendingRenderOp {
    pub target_view_id: u64,
    pub command_buffer: CommandBuffer,
    pub instance_id: u32,
}

fn get_ui_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("VeldMap UI BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn get_ui_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

pub fn execute_render_commands<'a>(
    rp: &mut wgpu::RenderPass<'a>,
    command_buffer: &'a CommandBuffer,
    resources: &'a ResourceManager,
    target_width: u32,
    target_height: u32,
    requestor_id: u32,
) -> anyhow::Result<()> {
    for wgpu_cmd in &command_buffer.commands {
        let cmd = match &wgpu_cmd.command { Some(c) => c, None => continue };
        match cmd {
            WgpuCommand::SetPipeline(p) => {
                if let Some(Resource::GpuObj(GpuObject::RenderPipeline(pipeline))) = resources.get_resource(p.pipeline_id, requestor_id) {
                    rp.set_pipeline(pipeline.as_ref());
                }
            }
            WgpuCommand::SetBindGroup(bg) => {
                if let Some(Resource::GpuObj(GpuObject::BindGroup(bind_group))) = resources.get_resource(bg.bind_group_id, requestor_id) {
                    rp.set_bind_group(bg.index, bind_group.as_ref(), &bg.dynamic_offsets);
                } else if let Some(Resource::Data(region_id)) = resources.get_resource(bg.bind_group_id, requestor_id) {
                    // Fallback: texture region → create ad-hoc bind group
                    if let Some((texture, _, _, _)) = resources.get_texture_info(region_id) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let bgl = get_ui_layout(&resources.get_device());
                        let sampler = get_ui_sampler(&resources.get_device());
                        let bg_res = resources.get_device().create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Proxy Fallback BG"), layout: &bgl,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                            ],
                        });
                        rp.set_bind_group(bg.index, &bg_res, &[]);
                    }
                }
            }
            WgpuCommand::SetVertexBuffer(vb) => {
                if let Some(buffer) = resources.get_buffer(vb.buffer_id) {
                    let end = if vb.size > 0 { (vb.offset + vb.size).min(buffer.size()) } else { buffer.size() };
                    rp.set_vertex_buffer(vb.slot, buffer.slice(vb.offset..end));
                }
            }
            WgpuCommand::SetIndexBuffer(ib) => {
                let format = if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                if let Some(buffer) = resources.get_buffer(ib.buffer_id) {
                    let end = if ib.size > 0 { (ib.offset + ib.size).min(buffer.size()) } else { buffer.size() };
                    rp.set_index_buffer(buffer.slice(ib.offset..end), format);
                }
            }
            WgpuCommand::Draw(d) => {
                rp.draw(d.first_vertex..(d.first_vertex + d.vertex_count), d.first_instance..(d.first_instance + d.instance_count));
            }
            WgpuCommand::DrawIndexed(di) => {
                rp.draw_indexed(di.first_index..(di.first_index + di.index_count), di.base_vertex, di.first_instance..(di.first_instance + di.instance_count));
            }
            WgpuCommand::SetViewport(v) => {
                let x = v.x.clamp(0.0, target_width as f32);
                let y = v.y.clamp(0.0, target_height as f32);
                let w = v.width.min(target_width as f32 - x);
                let h = v.height.min(target_height as f32 - y);
                if w > 0.0 && h > 0.0 { rp.set_viewport(x, y, w, h, v.min_depth, v.max_depth); }
            }
            WgpuCommand::SetScissorRect(s) => {
                let x = s.x.min(target_width.saturating_sub(1));
                let y = s.y.min(target_height.saturating_sub(1));
                let w = s.width.min(target_width - x).max(1);
                let h = s.height.min(target_height - y).max(1);
                rp.set_scissor_rect(x, y, w, h);
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct ComputeService {
    resources: Arc<ResourceManager>,
}

impl ComputeService {
    pub fn new(resources: Arc<ResourceManager>) -> Self { Self { resources } }

    pub fn build_encoder(&self, req: Submit, requestor_id: u32) -> anyhow::Result<wgpu::CommandEncoder> {
        let device = self.resources.get_device();
        let view = match self.resources.get_resource(req.target_texture_view_id, requestor_id) {
            Some(Resource::GpuObj(GpuObject::TextureView(v))) => v,
            _ => return Err(anyhow::anyhow!("Target view {} not found", req.target_texture_view_id)),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Compute Execute") });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Compute Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None, multiview_mask: None, timestamp_writes: None, occlusion_query_set: None,
            });
            if let Some(cb) = &req.command_buffer {
                let _ = execute_render_commands(&mut rp, cb, &self.resources, 2048, 2048, requestor_id);
            }
        }
        Ok(encoder)
    }

    pub fn create_resource(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        let req = ComputeResourceRequest::decode(&payload[..])?;
        let mut handle = ResourceHandle::default();

        match req.command {
            Some(ComputeCommand::CreateShader(s)) => {
                handle.id = self.resources.create_shader(&s.source, Some(&s.label), requestor_id);
            }
            Some(ComputeCommand::CreatePipeline(p)) => {
                handle.id = self.resources.create_pipeline(&p, requestor_id)?;
            }
            Some(ComputeCommand::CreateSampler(s)) => {
                handle.id = self.resources.create_sampler(s.mag_filter as i32, s.min_filter as i32, requestor_id);
            }
            Some(ComputeCommand::CreateTextureView(tv)) => {
                handle.id = self.resources.create_texture_view(tv.texture_id, requestor_id)?;
            }
            Some(ComputeCommand::CreateBindGroupLayout(bgl)) => {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = bgl.entries.iter().map(|e| {
                    let visibility = wgpu::ShaderStages::from_bits_truncate(e.visibility);
                    let ty = match &e.ty {
                        Some(Ty::Buffer(b)) => wgpu::BindingType::Buffer {
                            ty: match b.r#type {
                                1 => wgpu::BufferBindingType::Uniform,
                                2 => wgpu::BufferBindingType::Storage { read_only: false },
                                3 => wgpu::BufferBindingType::Storage { read_only: true },
                                _ => wgpu::BufferBindingType::Uniform,
                            },
                            has_dynamic_offset: b.has_dynamic_offset, min_binding_size: None,
                        },
                        Some(Ty::Sampler(s)) => wgpu::BindingType::Sampler(match s.r#type {
                            1 => wgpu::SamplerBindingType::Filtering,
                            2 => wgpu::SamplerBindingType::NonFiltering,
                            3 => wgpu::SamplerBindingType::Comparison,
                            _ => wgpu::SamplerBindingType::Filtering,
                        }),
                        Some(Ty::Texture(t)) => wgpu::BindingType::Texture {
                            sample_type: match t.sample_type {
                                1 => wgpu::TextureSampleType::Float { filterable: true },
                                2 => wgpu::TextureSampleType::Float { filterable: false },
                                3 => wgpu::TextureSampleType::Depth,
                                4 => wgpu::TextureSampleType::Uint,
                                5 => wgpu::TextureSampleType::Sint,
                                _ => wgpu::TextureSampleType::Float { filterable: true },
                            },
                            view_dimension: match t.view_dimension {
                                1 => wgpu::TextureViewDimension::D1,
                                2 => wgpu::TextureViewDimension::D2,
                                3 => wgpu::TextureViewDimension::D2Array,
                                4 => wgpu::TextureViewDimension::Cube,
                                5 => wgpu::TextureViewDimension::CubeArray,
                                6 => wgpu::TextureViewDimension::D3,
                                _ => wgpu::TextureViewDimension::D2,
                            },
                            multisampled: t.multisampled,
                        },
                        None => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None,
                        },
                    };
                    wgpu::BindGroupLayoutEntry { binding: e.binding, visibility, ty, count: None }
                }).collect();
                handle.id = self.resources.create_bind_group_layout(&entries, requestor_id);
            }
            Some(ComputeCommand::CreateBindGroup(bg)) => {
                handle.id = self.resources.create_bind_group(bg.layout_id, &bg.entries, requestor_id)?;
            }
            _ => return Err(anyhow::anyhow!("Unsupported resource command")),
        }
        Ok(ComputeResourceResponse { handle: Some(handle), error: String::new(), correlation_id: String::new() }.encode_to_vec())
    }

    pub fn execute(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        let req = Submit::decode(&payload[..])?;
        if let Some(cb) = req.command_buffer {
            let mut ops = PENDING_OPS.lock().unwrap();
            ops.push(PendingRenderOp { target_view_id: req.target_texture_view_id, command_buffer: cb, instance_id: requestor_id });
        }
        Ok(Vec::new())
    }
}
