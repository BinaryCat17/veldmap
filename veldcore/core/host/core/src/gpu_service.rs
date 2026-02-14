use crate::dispatcher::NativeService;
use crate::resources::ResourceManager;
use crate::wgpu::{GpuResourceRequest, GpuResourceResponse};
use crate::core::ResourceHandle;
use prost::Message;
use std::sync::Arc;
use image::GenericImageView;

pub fn execute_render_commands<'a>(
    rp: &mut wgpu::RenderPass<'a>,
    command_buffer: &'a crate::wgpu::CommandBuffer,
    resources: &'a crate::resources::ResourceManager,
) -> anyhow::Result<()> {
    use crate::wgpu::wgpu_command::Command;

    for wgpu_cmd in &command_buffer.commands {
        let cmd = match &wgpu_cmd.command {
            Some(c) => c,
            None => continue,
        };

        match cmd {
            Command::SetPipeline(p) => {
                if let Some(crate::resources::Resource::RenderPipeline(pipeline)) = resources.get_resource(p.pipeline_id) {
                    rp.set_pipeline(pipeline.as_ref());
                }
            }
            Command::SetBindGroup(bg) => {
                if let Some(crate::resources::Resource::BindGroup(bind_group)) = resources.get_resource(bg.bind_group_id) {
                    rp.set_bind_group(bg.index, bind_group.as_ref(), &bg.dynamic_offsets);
                } else {
                    // Fallback for direct texture binding (common in simple plugins)
                    if let Some(crate::resources::Resource::Texture { texture, .. }) = resources.get_resource(bg.bind_group_id) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let bgl = resources.get_ui_layout();
                        let sampler = resources.get_ui_sampler();
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
            Command::SetVertexBuffer(vb) => {
                if let Some(crate::resources::Resource::Buffer(buf)) = resources.get_resource(vb.buffer_id) {
                    let end = if vb.size > 0 { (vb.offset + vb.size).min(buf.size()) } else { buf.size() };
                    rp.set_vertex_buffer(vb.slot, buf.slice(vb.offset..end));
                }
            }
            Command::SetIndexBuffer(ib) => {
                let format = if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                if let Some(crate::resources::Resource::Buffer(buf)) = resources.get_resource(ib.buffer_id) {
                    let end = if ib.size > 0 { (ib.offset + ib.size).min(buf.size()) } else { buf.size() };
                    rp.set_index_buffer(buf.slice(ib.offset..end), format);
                }
            }
            Command::Draw(d) => {
                rp.draw(d.first_vertex..(d.first_vertex + d.vertex_count), d.first_instance..(d.first_instance + d.instance_count));
            }
            Command::DrawIndexed(di) => {
                rp.draw_indexed(di.first_index..(di.first_index + di.index_count), di.base_vertex, di.first_instance..(di.first_instance + di.instance_count));
            }
            Command::SetViewport(v) => {
                rp.set_viewport(v.x, v.y, v.width, v.height, v.min_depth, v.max_depth);
            }
            Command::SetScissorRect(s) => {
                rp.set_scissor_rect(s.x, s.y, s.width, s.height);
            }
        }
    }
    Ok(())
}

pub struct GpuService {
    resources: Arc<ResourceManager>,
}

impl GpuService {
    pub fn new(resources: Arc<ResourceManager>) -> Self {
        Self { resources }
    }

    fn submit(&self, req: crate::wgpu::Submit) -> anyhow::Result<()> {
        let view = if req.target_texture_view_id == 0 {
            return Err(anyhow::anyhow!("Submit to ID 0 (Surface) not supported in headless mode or without explicit context"));
        } else {
            match self.resources.get_resource(req.target_texture_view_id) {
                Some(crate::resources::Resource::TextureView(v)) => v,
                Some(crate::resources::Resource::Texture { texture, .. }) => Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default())),
                _ => return Err(anyhow::anyhow!("Target texture view not found")),
            }
        };

        let device = self.resources.get_device();
        let queue = self.resources.get_queue();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Wasm-Submit-Encoder") });
        
        {
            let clear = req.clear_color.unwrap_or_default();
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Wasm-Submit-RP"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear.r as f64, g: clear.g as f64, b: clear.b as f64, a: clear.a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            if let Some(cb) = &req.command_buffer {
                execute_render_commands(&mut rp, cb, &self.resources)?;
            }
        }

        {
            let q = queue.lock().unwrap();
            q.submit(Some(encoder.finish()));
            device.poll(wgpu::Maintain::Wait);
        }
        Ok(())
    }
}

impl NativeService for GpuService {
    fn call(&self, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match method {
            "submit" => {
                let req = crate::wgpu::Submit::decode(&payload[..])?;
                self.submit(req)?;
                Ok(Vec::new())
            }
            "create_resource" => {
                let req = GpuResourceRequest::decode(&payload[..])?;
                let mut handle = ResourceHandle::default();
                match req.command {
                    Some(crate::wgpu::gpu_resource_request::Command::CreateTexture(t)) => {
                        handle.id = self.resources.create_texture(t.width, t.height, t.format, t.usage, t.readonly);
                        handle.size = (t.width * t.height * 4) as u64; 
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateBuffer(b)) => {
                        handle.id = self.resources.create_buffer_ext(b.size, b.usage, b.mapped_at_creation, b.readonly);
                        handle.size = b.size;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateShader(s)) => {
                        handle.id = self.resources.create_shader(&s.source, Some(&s.label));
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreatePipeline(p)) => {
                        handle.id = self.resources.create_pipeline(p.shader_id, Some(&p.label), p.target_format, p.vertex_layouts)?;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateSampler(s)) => {
                        handle.id = self.resources.create_sampler(s.mag_filter, s.min_filter);
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateTextureView(tv)) => {
                        handle.id = self.resources.create_texture_view(tv.texture_id)?;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateBindGroupLayout(bgl)) => {
                        let mut entries = Vec::new();
                        for e in bgl.entries {
                            let visibility = wgpu::ShaderStages::from_bits_truncate(e.visibility);
                            let ty = match e.ty {
                                Some(crate::wgpu::bind_group_layout_entry::Ty::Buffer(b)) => {
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
                                Some(crate::wgpu::bind_group_layout_entry::Ty::Sampler(s)) => {
                                    wgpu::BindingType::Sampler(match s.r#type {
                                        1 => wgpu::SamplerBindingType::Filtering,
                                        2 => wgpu::SamplerBindingType::NonFiltering,
                                        3 => wgpu::SamplerBindingType::Comparison,
                                        _ => wgpu::SamplerBindingType::Filtering,
                                    })
                                }
                                Some(crate::wgpu::bind_group_layout_entry::Ty::Texture(t)) => {
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
                        handle.id = self.resources.create_bind_group_layout(&entries);
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::CreateBindGroup(bg)) => {
                        handle.id = self.resources.create_bind_group(bg.layout_id, &bg.entries)?;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::FsReadToBuffer(req)) => {
                        let data = std::fs::read(&req.path)?;
                        handle.id = self.resources.create_buffer_with_data(&data, req.usage, true);
                        handle.size = data.len() as u64;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::FsReadToTexture(req)) => {
                        let _data = std::fs::read(&req.path)?;
                        // Упрощенно: считаем что это сырые данные RGBA8 для текстуры 
                        // В реальности тут нужно знать размеры. 
                        // Поэтому ImageLoadToTexture полезнее.
                        return Err(anyhow::anyhow!("FsReadToTexture requires dimensions. Use ImageLoadToTexture instead."));
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::ImageLoadToTexture(req)) => {
                        let img = image::open(&req.path)?;
                        let (w, h) = img.dimensions();
                        let rgba = img.to_rgba8();
                        
                        handle.id = self.resources.create_texture(w, h, 0, req.usage, true);
                        self.resources.write_resource(handle.id, 0, &rgba)?;
                        handle.size = (w * h * 4) as u64;
                    }
                    Some(crate::wgpu::gpu_resource_request::Command::FreezeResource(id)) => {
                        if !self.resources.freeze_resource(id) {
                            return Err(anyhow::anyhow!("Resource {} not found to freeze", id));
                        }
                        handle.id = id;
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported resource command")),
                }
                Ok(GpuResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
            }
            _ => Err(anyhow::anyhow!("Unknown GPU method: {}", method)),
        }
    }
}
