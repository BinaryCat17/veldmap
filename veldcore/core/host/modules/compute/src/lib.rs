use veldmap_host_core::dispatcher::{AsyncNativeService, Dispatcher};
use veldmap_host_core::resources::{ResourceManager, Resource};
use veldmap_host_core::core::ResourceHandle;
use veldmap_host_core::compute::{
    ComputeResourceRequest, ComputeResourceResult, ComputeExecuteResult, Submit, CommandBuffer,
    compute_resource_request::Command as ComputeCommand,
    wgpu_command::Command as WgpuCommand,
    CreateTexture, CreateBuffer, CreateShaderModule, CreateRenderPipeline,
    CreateSampler, CreateTextureView, CreateBindGroupLayout, CreateBindGroup,
    bind_group_layout_entry::Ty, BufferBindingLayout, SamplerBindingLayout,
    TextureBindingLayout, StepMode, VertexAttribute, VertexBufferLayout,
    PrimitiveTopology, FrontFace, CullMode, IndexFormat, BindGroupEntry,
    bind_group_entry, FilterMode, TextureFormat,
};
use image::GenericImageView;
use prost::Message;
use std::sync::{Arc, Mutex};

// Global pending render ops queue - main thread will execute these
pub static PENDING_OPS: Mutex<Vec<PendingRenderOp>> = Mutex::new(Vec::new());

pub struct PendingRenderOp {
    pub target_view_id: u64,
    pub command_buffer: CommandBuffer,
    pub instance_id: u32,
}

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
    command_buffer: &'a CommandBuffer,
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
                                wgpu::BindingGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                wgpu::BindingGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
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
    dispatcher: Arc<Dispatcher>,
    resources: Arc<ResourceManager>,
}

impl ComputeService {
    pub fn new(dispatcher: Arc<Dispatcher>, resources: Arc<ResourceManager>) -> Self {
        Self { dispatcher, resources }
    }

    /// Build command encoder for execution (must be submitted by caller from main thread)
    pub fn build_encoder(&self, req: Submit, requestor_id: u32) -> anyhow::Result<wgpu::CommandEncoder> {
        let device = self.resources.get_device();
        
        let target_view = self.resources.get_resource(req.target_texture_view_id, requestor_id)
            .ok_or_else(|| anyhow::anyhow!("Target texture view {} not found", req.target_texture_view_id))?;
        
        let view = match target_view {
            Resource::TextureView(view) => view,
            _ => return Err(anyhow::anyhow!("Resource {} is not a texture view", req.target_texture_view_id)),
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { 
            label: Some("Compute Execute") 
        });

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Compute Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if let Some(cb) = &req.command_buffer {
                let _ = execute_render_commands(&mut rp, cb, &self.resources, 2048, 2048, requestor_id);
            }
        }

        Ok(encoder)
    }

    async fn handle_execute(&self, payload: Vec<u8>, requestor_id: u32) {
        let req = match Submit::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode Submit: {}", e);
                let correlation_id = String::new();
                let result = ComputeExecuteResult { error: e.to_string(), correlation_id };
                self.dispatcher.publish("compute/execute_result", result.encode_to_vec());
                return;
            }
        };
        let correlation_id = req.correlation_id.clone();
        if let Some(cb) = req.command_buffer {
            let mut ops = PENDING_OPS.lock().unwrap();
            ops.push(PendingRenderOp {
                target_view_id: req.target_texture_view_id,
                command_buffer: cb,
                instance_id: requestor_id,
            });
        }
        let result = ComputeExecuteResult { error: String::new(), correlation_id };
        self.dispatcher.publish("compute/execute_result", result.encode_to_vec());
    }

    async fn handle_create_resource(&self, payload: Vec<u8>, requestor_id: u32) {
        let req = match ComputeResourceRequest::decode(&payload[..]) {
            Ok(r) => r,
            Err(e) => {
                log::error!(target: "host", "Failed to decode ComputeResourceRequest: {}", e);
                let correlation_id = String::new();
                let result = ComputeResourceResult { handle: None, error: e.to_string(), correlation_id };
                self.dispatcher.publish("compute/create_resource_result", result.encode_to_vec());
                return;
            }
        };
        let correlation_id = req.correlation_id.clone();
        let mut handle = ResourceHandle::default();
        let instance_id = requestor_id;
        let mut error = String::new();

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
                if let Err(e) = self.resources.create_pipeline(&p, instance_id) {
                    error = e.to_string();
                }
            }
            Some(ComputeCommand::CreateSampler(s)) => {
                handle.id = self.resources.create_sampler(s.mag_filter as i32, s.min_filter as i32, instance_id);
            }
            Some(ComputeCommand::CreateTextureView(tv)) => {
                veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_COMPUTE, "[COMPUTE] Creating texture view for texture_id={}", tv.texture_id);
                match self.resources.create_texture_view(tv.texture_id, instance_id) {
                    Ok(id) => handle.id = id,
                    Err(e) => error = e.to_string(),
                }
                veldmap_host_core::vtrace!(veldmap_host_core::logging::FLAG_COMPUTE, "[COMPUTE] Created texture view: id={}", handle.id);
            }
            Some(ComputeCommand::CreateBindGroupLayout(bgl)) => {
                let mut entries = Vec::new();
                for e in bgl.entries {
                    let visibility = wgpu::ShaderStages::from_bits_truncate(e.visibility);
                    let ty = match e.ty {
                        Some(Ty::Buffer(b)) => {
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
                        Some(Ty::Sampler(s)) => {
                            wgpu::BindingType::Sampler(match s.r#type {
                                1 => wgpu::SamplerBindingType::Filtering,
                                2 => wgpu::SamplerBindingType::NonFiltering,
                                3 => wgpu::SamplerBindingType::Comparison,
                                _ => wgpu::SamplerBindingType::Filtering,
                            })
                        }
                        Some(Ty::Texture(t)) => {
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
                        None => { error = "Binding type missing".into(); }
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
                match self.resources.create_bind_group(bg.layout_id, &bg.entries, instance_id) {
                    Ok(id) => handle.id = id,
                    Err(e) => error = e.to_string(),
                }
            }
            Some(ComputeCommand::FsReadToBuffer(req_fs)) => {
                match std::fs::read(&req_fs.path) {
                    Ok(data) => {
                        handle.id = self.resources.create_buffer_with_data(&data, req_fs.usage, true, instance_id);
                        handle.size = data.len() as u64;
                    }
                    Err(e) => error = e.to_string(),
                }
            }
            Some(ComputeCommand::ImageLoadToTexture(req_img)) => {
                match image::open(&req_img.path) {
                    Ok(img) => {
                        let (w, h) = img.dimensions();
                        let rgba = img.to_rgba8();
                        handle.id = self.resources.create_texture(w, h, 0, req_img.usage, true, instance_id);
                        if let Err(e) = self.resources.write_resource(handle.id, 0, &rgba, instance_id) {
                            error = e.to_string();
                        }
                        handle.size = (w * h * 4) as u64;
                    }
                    Err(e) => error = e.to_string(),
                }
            }
            _ => error = "Unsupported resource command".into(),
        }

        let result = ComputeResourceResult {
            handle: if error.is_empty() { Some(handle) } else { None },
            error,
            correlation_id,
        };
        self.dispatcher.publish("compute/create_resource_result", result.encode_to_vec());
    }
}

#[async_trait::async_trait]
impl AsyncNativeService for ComputeService {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32) {
        match topic {
            "execute" => self.handle_execute(payload, requestor_id).await,
            "create_resource" => self.handle_create_resource(payload, requestor_id).await,
            _ => log::warn!(target: "host", "Unknown compute topic: {}", topic),
        }
    }
}
