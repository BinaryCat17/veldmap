use crate::dispatcher::NativeService;
use crate::resources::{ResourceManager, Resource};
use crate::core::ResourceHandle;
use crate::compute::{
    ComputeResourceRequest, ComputeResourceResponse, 
    compute_resource_request::Command as ComputeCommand,
    wgpu_command::Command as WgpuCommand
};
use image::GenericImageView;
use prost::Message;
use std::sync::{Arc, Mutex};

pub fn get_ui_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("VeldMap UI BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry { 
                binding: 0, 
                visibility: wgpu::ShaderStages::FRAGMENT, 
                ty: wgpu::BindingType::Texture { 
                    sample_type: wgpu::TextureSampleType::Float { filterable: true }, 
                    view_dimension: wgpu::TextureViewDimension::D2, 
                    multisampled: false 
                }, 
                count: None 
            },
            wgpu::BindGroupLayoutEntry { 
                binding: 1, 
                visibility: wgpu::ShaderStages::FRAGMENT, 
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), 
                count: None 
            },
        ],
    })
}

pub fn get_ui_sampler(device: &wgpu::Device) -> wgpu::Sampler {
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
    command_buffer: &'a crate::compute::CommandBuffer,
    resources: &'a ResourceManager,
    target_width: u32,
    target_height: u32,
    requestor_id: u32,
) -> anyhow::Result<()> {
    for wgpu_cmd in &command_buffer.commands {
        let cmd = match &wgpu_cmd.command {
            Some(c) => c,
            None => continue,
        };

        match cmd {
            WgpuCommand::SetPipeline(p) => {
                if let Some(Resource::RenderPipeline(pipeline)) = resources.get_resource(p.pipeline_id, requestor_id) {
                    rp.set_pipeline(pipeline.as_ref());
                }
            }
            WgpuCommand::SetBindGroup(bg) => {
                if let Some(Resource::BindGroup(bind_group)) = resources.get_resource(bg.bind_group_id, requestor_id) {
                    rp.set_bind_group(bg.index, bind_group.as_ref(), &bg.dynamic_offsets);
                } else {
                    if let Some(Resource::Texture { texture, .. }) = resources.get_resource(bg.bind_group_id, requestor_id) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let bgl = get_ui_layout(&resources.get_device());
                        let sampler = get_ui_sampler(&resources.get_device());
                        let bg_res = resources.get_device().create_bind_group(&wgpu::BindGroupDescriptor { 
                            label: Some("Proxy Fallback BG"), 
                            layout: &bgl, 
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
                            ] 
                        });
                        rp.set_bind_group(bg.index, &bg_res, &[]);
                    }
                }
            }
            WgpuCommand::SetVertexBuffer(vb) => {
                if let Some(Resource::Buffer(buf)) = resources.get_resource(vb.buffer_id, requestor_id) {
                    let end = if vb.size > 0 { (vb.offset + vb.size).min(buf.size()) } else { buf.size() };
                    rp.set_vertex_buffer(vb.slot, buf.slice(vb.offset..end));
                }
            }
            WgpuCommand::SetIndexBuffer(ib) => {
                let format = if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                if let Some(Resource::Buffer(buf)) = resources.get_resource(ib.buffer_id, requestor_id) {
                    let end = if ib.size > 0 { (ib.offset + ib.size).min(buf.size()) } else { buf.size() };
                    rp.set_index_buffer(buf.slice(ib.offset..end), format);
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
                if w > 0.0 && h > 0.0 {
                    rp.set_viewport(x, y, w, h, v.min_depth, v.max_depth);
                }
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

pub struct ComputeService {
    resources: Arc<ResourceManager>,
    render_queue: Arc<Mutex<Vec<crate::compute::Submit>>>,
}

impl ComputeService {
    pub fn new(
        resources: Arc<ResourceManager>, 
        render_queue: Arc<Mutex<Vec<crate::compute::Submit>>>,
    ) -> Self {
        Self { resources, render_queue }
    }

    fn submit(&self, req: crate::compute::Submit) -> anyhow::Result<()> {
        let mut queue = self.render_queue.lock().unwrap();
        queue.push(req);
        Ok(())
    }
}

impl NativeService for ComputeService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "submit" => {
                let mut req = crate::compute::Submit::decode(&payload[..])?;
                req.instance_id = requestor_id; // Override with verified ID
                self.submit(req)?;
                Ok(Vec::new())
            }
            "create_resource" => {
                let req = ComputeResourceRequest::decode(&payload[..])?;
                let mut handle = ResourceHandle::default();
                let instance_id = requestor_id;

                match req.command {
                    Some(ComputeCommand::CreateTexture(t)) => {
                        handle.id = self.resources.create_texture(t.width, t.height, t.format as i32, t.usage, t.readonly, instance_id);
                        handle.size = (t.width * t.height * 4) as u64; 
                    }
                    Some(ComputeCommand::CreateBuffer(b)) => {
                        handle.id = self.resources.create_buffer_ext(b.size, b.usage, b.mapped_at_creation, b.readonly, instance_id);
                        handle.size = b.size;
                    }
                    Some(ComputeCommand::CreateShader(s)) => {
                        handle.id = self.resources.create_shader(&s.source, Some(&s.label), instance_id);
                    }
                    Some(ComputeCommand::CreatePipeline(p)) => {
                        handle.id = self.resources.create_pipeline(&p, instance_id)?;
                    }
                    Some(ComputeCommand::CreateSampler(s)) => {
                        handle.id = self.resources.create_sampler(s.mag_filter as i32, s.min_filter as i32, instance_id);
                    }
                    Some(ComputeCommand::CreateTextureView(tv)) => {
                        handle.id = self.resources.create_texture_view(tv.texture_id, instance_id)?;
                    }
                    Some(ComputeCommand::CreateBindGroupLayout(bgl)) => {
                        let mut entries = Vec::new();
                        for e in bgl.entries {
                            let visibility = wgpu::ShaderStages::from_bits_truncate(e.visibility);
                            let ty = match e.ty {
                                Some(crate::compute::bind_group_layout_entry::Ty::Buffer(b)) => {
                                    wgpu::BindingType::Buffer {
                                        ty: match b.r#type {
                                            1 => wgpu::BufferBindingType::Uniform,
                                            2 => wgpu::BufferBindingType::Storage { read_only: false },
                                            3 => wgpu::BufferBindingType::Storage { read_only: true },
                                            _ => wgpu::BufferBindingType::Uniform,
                                        },
                                        has_dynamic_offset: b.has_dynamic_offset,
                                        min_binding_size: None,
                                    }
                                }
                                Some(crate::compute::bind_group_layout_entry::Ty::Sampler(s)) => {
                                    wgpu::BindingType::Sampler(match s.r#type {
                                        1 => wgpu::SamplerBindingType::Filtering,
                                        2 => wgpu::SamplerBindingType::NonFiltering,
                                        3 => wgpu::SamplerBindingType::Comparison,
                                        _ => wgpu::SamplerBindingType::Filtering,
                                    })
                                }
                                Some(crate::compute::bind_group_layout_entry::Ty::Texture(t)) => {
                                    wgpu::BindingType::Texture {
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
                                    }
                                }
                                None => return Err(anyhow::anyhow!("Binding type missing")),
                            };
                            entries.push(wgpu::BindGroupLayoutEntry {
                                binding: e.binding,
                                visibility,
                                ty,
                                count: None,
                            });
                        }
                        handle.id = self.resources.create_bind_group_layout(&entries, instance_id);
                    }
                    Some(ComputeCommand::CreateBindGroup(bg)) => {
                        handle.id = self.resources.create_bind_group(bg.layout_id, &bg.entries, instance_id)?;
                    }
                    Some(ComputeCommand::FsReadToBuffer(req_fs)) => {
                        let data = std::fs::read(&req_fs.path)?;
                        handle.id = self.resources.create_buffer_with_data(&data, req_fs.usage, true, instance_id);
                        handle.size = data.len() as u64;
                    }
                    Some(ComputeCommand::ImageLoadToTexture(req_img)) => {
                        let img = image::open(&req_img.path)?;
                        let (w, h) = img.dimensions();
                        let rgba = img.to_rgba8();
                        
                        handle.id = self.resources.create_texture(w, h, 0, req_img.usage, true, instance_id);
                        self.resources.write_resource(handle.id, 0, &rgba, instance_id)?;
                        handle.size = (w * h * 4) as u64;
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported resource command")),
                }
                Ok(ComputeResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            _ => Err(anyhow::anyhow!("Unknown Compute method: {}", method)),
        }
    }
}
