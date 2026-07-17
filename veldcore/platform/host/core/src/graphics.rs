use std::sync::Arc;
use std::sync::Mutex;
use dashmap::DashMap;
use crate::memory::{MemoryManager, proto_to_wgpu_format, surface_format_to_proto};
use crate::registry::{ResourceRegistry, ResourceId, ResourceBackend, Access};
use crate::core::ResourceHandle;
use crate::compute::{
    ComputeResourceRequest, ComputeResourceResponse, Submit, CommandBuffer,
    compute_resource_request::Command as ComputeCommand,
    wgpu_command::Command as WgpuCommand,
    bind_group_layout_entry::Ty,
    VertexFormat, StepMode, FilterMode,
    PrimitiveTopology, BlendFactor, BlendOperation, FrontFace, CullMode,
};
use prost::Message;

// ── Unified Resource Enum ──────────────────────────────────────

#[derive(Clone)]
pub enum Resource {
    /// Data-bearing: bytes live in MemoryManager
    Data(ResourceId),
    /// Opaque GPU objects: no shareable bytes, just Arc-wrapped wgpu handles
    GpuObj(GpuObject),
}

#[derive(Clone)]
pub enum GpuObject {
    TextureView(Arc<wgpu::TextureView>),
    Sampler(Arc<wgpu::Sampler>),
    BindGroupLayout(Arc<wgpu::BindGroupLayout>),
    RenderPipeline(Arc<wgpu::RenderPipeline>),
    BindGroup(Arc<wgpu::BindGroup>),
    ShaderModule(Arc<wgpu::ShaderModule>),
}

// ── GPU Object Registry Entry ──────────────────────────────────

struct GpuEntry {
    obj: GpuObject,
}

// ── Render command queue ───────────────────────────────────────

pub struct PendingRenderOp {
    pub target_view_id: u64,
    pub command_buffer: CommandBuffer,
    pub instance_id: u32,
}

// ── GraphicsDevice ─────────────────────────────────────────────

pub struct GraphicsDevice {
    registry: Arc<ResourceRegistry>,
    memory: Arc<MemoryManager>,
    gpu_objects: DashMap<ResourceId, GpuEntry>,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
    surface_format: wgpu::TextureFormat,
    /// Render ops submitted by modules, drained by the runner's frame loop.
    pending_ops: Mutex<Vec<PendingRenderOp>>,
}

impl GraphicsDevice {
    pub fn new(
        registry: Arc<ResourceRegistry>,
        memory: Arc<MemoryManager>,
        device: Arc<wgpu::Device>,
        queue: Arc<std::sync::Mutex<wgpu::Queue>>,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            registry,
            memory,
            gpu_objects: DashMap::new(),
            device,
            queue,
            surface_format,
            pending_ops: Mutex::new(Vec::new()),
        }
    }

    /// Drain render ops queued by modules since the last frame.
    pub fn take_pending_ops(&self) -> Vec<PendingRenderOp> {
        std::mem::take(&mut *self.pending_ops.lock().unwrap())
    }

    pub fn registry(&self) -> &Arc<ResourceRegistry> { &self.registry }
    pub fn memory(&self) -> &Arc<MemoryManager> { &self.memory }
    pub fn get_device(&self) -> Arc<wgpu::Device> { self.device.clone() }
    pub fn get_queue(&self) -> Arc<std::sync::Mutex<wgpu::Queue>> { self.queue.clone() }
    pub fn get_surface_format_proto(&self) -> i32 { surface_format_to_proto(self.surface_format) }

    // ── GPU object helpers ────────────────────────────────────

    fn insert_gpu(&self, obj: GpuObject, owner_id: u32, name: Option<String>) -> ResourceId {
        let id = self.registry.register(ResourceBackend::GpuState, owner_id, name);
        self.gpu_objects.insert(id, GpuEntry { obj });
        id
    }

    fn get_gpu(&self, id: ResourceId, requestor_id: u32) -> anyhow::Result<GpuObject> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return Err(anyhow::anyhow!("Access denied to GPU object {}", id));
        }
        self.gpu_objects.get(&id)
            .map(|e| e.obj.clone())
            .ok_or_else(|| anyhow::anyhow!("GPU object {} not found", id))
    }

    // ── Unified lookup ────────────────────────────────────────

    pub fn get_resource(&self, id: ResourceId, requestor_id: u32) -> Option<Resource> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return None;
        }
        let backend = self.registry.get_backend(id)?;
        match backend {
            ResourceBackend::GpuState => {
                self.gpu_objects.get(&id).map(|e| Resource::GpuObj(e.obj.clone()))
            }
            ResourceBackend::Memory => {
                if self.memory.exists(id) {
                    Some(Resource::Data(id))
                } else {
                    None
                }
            }
        }
    }

    pub fn get_buffer(&self, id: ResourceId, requestor_id: u32) -> Option<Arc<wgpu::Buffer>> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return None;
        }
        self.memory.get_buffer(id)
    }

    pub fn get_texture_info(&self, id: ResourceId, requestor_id: u32) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return None;
        }
        self.memory.get_texture(id)
    }

    // ── GPU object creation ───────────────────────────────────

    pub fn create_texture_view(&self, texture_id: ResourceId, owner_id: u32) -> anyhow::Result<ResourceId> {
        let (texture, _, _, _) = self.get_texture_info(texture_id, owner_id)
            .ok_or_else(|| anyhow::anyhow!("Texture region {} not found or access denied", texture_id))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(self.insert_gpu(GpuObject::TextureView(Arc::new(view)), owner_id, None))
    }

    pub fn create_sampler(&self, mag_proto: i32, min_proto: i32, owner_id: u32) -> ResourceId {
        let mag = match FilterMode::try_from(mag_proto).unwrap_or(FilterMode::FiltLinear) {
            FilterMode::FiltNearest => wgpu::FilterMode::Nearest,
            FilterMode::FiltLinear => wgpu::FilterMode::Linear,
        };
        let min = match FilterMode::try_from(min_proto).unwrap_or(FilterMode::FiltLinear) {
            FilterMode::FiltNearest => wgpu::FilterMode::Nearest,
            FilterMode::FiltLinear => wgpu::FilterMode::Linear,
        };
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: mag, min_filter: min, ..Default::default()
        });
        self.insert_gpu(GpuObject::Sampler(Arc::new(sampler)), owner_id, None)
    }

    pub fn create_bind_group_layout(&self, entries: &[wgpu::BindGroupLayoutEntry], owner_id: u32) -> ResourceId {
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries,
        });
        self.insert_gpu(GpuObject::BindGroupLayout(Arc::new(layout)), owner_id, None)
    }

    pub fn create_shader(&self, source: &str, label: Option<&str>, owner_id: u32) -> ResourceId {
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label, source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.insert_gpu(GpuObject::ShaderModule(Arc::new(shader)), owner_id, None)
    }

    pub fn create_bind_group(&self, layout_id: ResourceId, entries_proto: &[crate::compute::BindGroupEntry], owner_id: u32) -> anyhow::Result<ResourceId> {
        let layout = match self.get_gpu(layout_id, owner_id)? {
            GpuObject::BindGroupLayout(l) => l,
            _ => return Err(anyhow::anyhow!("Object {} is not a BGL", layout_id)),
        };

        let mut keep_buffers: Vec<(u32, Arc<wgpu::Buffer>)> = Vec::new();
        let mut keep_views: Vec<(u32, Arc<wgpu::TextureView>)> = Vec::new();
        let mut keep_samplers: Vec<(u32, Arc<wgpu::Sampler>)> = Vec::new();

        for e in entries_proto {
            match &e.resource {
                Some(crate::compute::bind_group_entry::Resource::BufferId(bid)) => {
                    let b = self.get_buffer(*bid, owner_id)
                        .ok_or_else(|| anyhow::anyhow!("Buffer {} not found or access denied", bid))?;
                    keep_buffers.push((e.binding, b));
                }
                Some(crate::compute::bind_group_entry::Resource::BufferBinding(bb)) => {
                    let b = self.get_buffer(bb.buffer_id, owner_id)
                        .ok_or_else(|| anyhow::anyhow!("Buffer {} not found or access denied", bb.buffer_id))?;
                    keep_buffers.push((e.binding, b));
                }
                Some(crate::compute::bind_group_entry::Resource::TextureViewId(tvid)) => {
                    match self.get_gpu(*tvid, owner_id) {
                        Ok(GpuObject::TextureView(tv)) => { keep_views.push((e.binding, tv)); }
                        _ => {
                            if let Some((tex, _, _, _)) = self.get_texture_info(*tvid, owner_id) {
                                let v = Arc::new(tex.create_view(&wgpu::TextureViewDescriptor::default()));
                                keep_views.push((e.binding, v));
                            } else {
                                return Err(anyhow::anyhow!("TextureView {} not found or access denied", tvid));
                            }
                        }
                    }
                }
                Some(crate::compute::bind_group_entry::Resource::SamplerId(sid)) => {
                    let s = match self.get_gpu(*sid, owner_id)? {
                        GpuObject::Sampler(s) => s,
                        _ => return Err(anyhow::anyhow!("Object {} is not a Sampler", sid)),
                    };
                    keep_samplers.push((e.binding, s));
                }
                None => {}
            }
        }

        let mut entries = Vec::new();
        for e in entries_proto {
            let resource = match &e.resource {
                Some(crate::compute::bind_group_entry::Resource::BufferId(_)) => {
                    let b = &keep_buffers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Buffer(b.as_entire_buffer_binding())
                }
                Some(crate::compute::bind_group_entry::Resource::BufferBinding(bb)) => {
                    let b = &keep_buffers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: b, offset: bb.offset, size: std::num::NonZeroU64::new(bb.size),
                    })
                }
                Some(crate::compute::bind_group_entry::Resource::TextureViewId(_)) => {
                    let tv = &keep_views.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::TextureView(tv)
                }
                Some(crate::compute::bind_group_entry::Resource::SamplerId(_)) => {
                    let s = &keep_samplers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Sampler(s)
                }
                None => continue,
            };
            entries.push(wgpu::BindGroupEntry { binding: e.binding, resource });
        }

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &layout, entries: &entries,
        });
        Ok(self.insert_gpu(GpuObject::BindGroup(Arc::new(bg)), owner_id, None))
    }

    pub fn create_pipeline(&self, req: &crate::compute::CreateRenderPipeline, owner_id: u32) -> anyhow::Result<ResourceId> {
        let shader = match self.get_gpu(req.shader_id, owner_id)? {
            GpuObject::ShaderModule(s) => s,
            _ => return Err(anyhow::anyhow!("Object {} is not a Shader", req.shader_id)),
        };

        let target_format = proto_to_wgpu_format(req.target_format);

        let mut bgl_refs = Vec::new();
        for &id in &req.bind_group_layout_ids {
            match self.get_gpu(id, owner_id)? {
                GpuObject::BindGroupLayout(l) => bgl_refs.push(l),
                _ => return Err(anyhow::anyhow!("Object {} is not a BGL", id)),
            }
        }

        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &bgl_refs.iter().map(|l| l.as_ref()).collect::<Vec<_>>(),
            immediate_size: 0,
        });

        let mut wgpu_vertex_layouts = Vec::new();
        let mut keep_attrs = Vec::new();
        for vl in &req.vertex_layouts {
            let attrs: Vec<wgpu::VertexAttribute> = vl.attributes.iter().map(|a| wgpu::VertexAttribute {
                offset: a.offset, shader_location: a.shader_location,
                format: match VertexFormat::try_from(a.format).unwrap_or(VertexFormat::VtxFloat32x2) {
                    VertexFormat::VtxFloat32 => wgpu::VertexFormat::Float32,
                    VertexFormat::VtxFloat32x2 => wgpu::VertexFormat::Float32x2,
                    VertexFormat::VtxFloat32x3 => wgpu::VertexFormat::Float32x3,
                    VertexFormat::VtxFloat32x4 => wgpu::VertexFormat::Float32x4,
                    VertexFormat::VtxUint32 => wgpu::VertexFormat::Uint32,
                },
            }).collect();
            keep_attrs.push(attrs);
        }
        for i in 0..req.vertex_layouts.len() {
            wgpu_vertex_layouts.push(wgpu::VertexBufferLayout {
                array_stride: req.vertex_layouts[i].array_stride,
                step_mode: if StepMode::try_from(req.vertex_layouts[i].step_mode).unwrap_or(StepMode::StepVertex) == StepMode::StepInstance {
                    wgpu::VertexStepMode::Instance
                } else {
                    wgpu::VertexStepMode::Vertex
                },
                attributes: &keep_attrs[i],
            });
        }

        let map_blend_factor = |f: i32| match BlendFactor::try_from(f).unwrap_or(BlendFactor::One) {
            BlendFactor::Zero => wgpu::BlendFactor::Zero,
            BlendFactor::One => wgpu::BlendFactor::One,
            BlendFactor::Src => wgpu::BlendFactor::Src,
            BlendFactor::OneMinusSrc => wgpu::BlendFactor::OneMinusSrc,
            BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
            BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
            BlendFactor::Dst => wgpu::BlendFactor::Dst,
            BlendFactor::OneMinusDst => wgpu::BlendFactor::OneMinusDst,
            BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
            BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        };
        let map_blend_op = |o: i32| match BlendOperation::try_from(o).unwrap_or(BlendOperation::Add) {
            BlendOperation::Add => wgpu::BlendOperation::Add,
            BlendOperation::Subtract => wgpu::BlendOperation::Subtract,
            BlendOperation::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
            BlendOperation::Min => wgpu::BlendOperation::Min,
            BlendOperation::Max => wgpu::BlendOperation::Max,
        };

        let blend = req.blend.as_ref().map(|b| wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: b.color.as_ref().map(|c| map_blend_factor(c.src_factor)).unwrap_or(wgpu::BlendFactor::One),
                dst_factor: b.color.as_ref().map(|c| map_blend_factor(c.dst_factor)).unwrap_or(wgpu::BlendFactor::Zero),
                operation: b.color.as_ref().map(|c| map_blend_op(c.operation)).unwrap_or(wgpu::BlendOperation::Add),
            },
            alpha: wgpu::BlendComponent {
                src_factor: b.alpha.as_ref().map(|c| map_blend_factor(c.src_factor)).unwrap_or(wgpu::BlendFactor::One),
                dst_factor: b.alpha.as_ref().map(|c| map_blend_factor(c.dst_factor)).unwrap_or(wgpu::BlendFactor::Zero),
                operation: b.alpha.as_ref().map(|c| map_blend_op(c.operation)).unwrap_or(wgpu::BlendOperation::Add),
            },
        }).or(Some(wgpu::BlendState::ALPHA_BLENDING));

        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: if req.label.is_empty() { None } else { Some(&req.label) },
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some(&req.vertex_entry),
                buffers: &wgpu_vertex_layouts, compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some(&req.fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format, blend, write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: match PrimitiveTopology::try_from(req.primitive_topology).unwrap_or(PrimitiveTopology::TopologyTriangleList) {
                    PrimitiveTopology::TopologyTriangleList => wgpu::PrimitiveTopology::TriangleList,
                    PrimitiveTopology::TopologyTriangleStrip => wgpu::PrimitiveTopology::TriangleStrip,
                    PrimitiveTopology::TopologyPointList => wgpu::PrimitiveTopology::PointList,
                    PrimitiveTopology::TopologyLineList => wgpu::PrimitiveTopology::LineList,
                    PrimitiveTopology::TopologyLineStrip => wgpu::PrimitiveTopology::LineStrip,
                },
                strip_index_format: if req.primitive_topology == PrimitiveTopology::TopologyTriangleStrip as i32
                    || req.primitive_topology == PrimitiveTopology::TopologyLineStrip as i32
                {
                    match crate::compute::IndexFormat::try_from(req.strip_index_format).unwrap_or(crate::compute::IndexFormat::IdxUint16) {
                        crate::compute::IndexFormat::IdxUint16 => Some(wgpu::IndexFormat::Uint16),
                        crate::compute::IndexFormat::IdxUint32 => Some(wgpu::IndexFormat::Uint32),
                    }
                } else { None },
                front_face: match FrontFace::try_from(req.front_face).unwrap_or(FrontFace::Ccw) {
                    FrontFace::Ccw => wgpu::FrontFace::Ccw,
                    FrontFace::Cw => wgpu::FrontFace::Cw,
                },
                cull_mode: match CullMode::try_from(req.cull_mode).unwrap_or(CullMode::None) {
                    CullMode::None => None,
                    CullMode::Front => Some(wgpu::Face::Front),
                    CullMode::Back => Some(wgpu::Face::Back),
                },
                unclipped_depth: false, polygon_mode: wgpu::PolygonMode::Fill, conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(self.insert_gpu(GpuObject::RenderPipeline(Arc::new(pipeline)), owner_id, None))
    }

    // ── Compute: create resource (protobuf dispatch) ──────────

    pub fn create_resource(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        let req = ComputeResourceRequest::decode(&payload[..])?;
        let mut handle = ResourceHandle::default();

        match req.command {
            Some(ComputeCommand::CreateShader(s)) => {
                let name = if s.label.is_empty() { None } else { Some(s.label.clone()) };
                let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: name.as_deref(), source: wgpu::ShaderSource::Wgsl(s.source.into()),
                });
                handle.id = self.insert_gpu(GpuObject::ShaderModule(Arc::new(shader)), requestor_id, name);
            }
            Some(ComputeCommand::CreatePipeline(p)) => {
                handle.id = self.create_pipeline(&p, requestor_id)?;
            }
            Some(ComputeCommand::CreateSampler(s)) => {
                handle.id = self.create_sampler(s.mag_filter as i32, s.min_filter as i32, requestor_id);
            }
            Some(ComputeCommand::CreateTextureView(tv)) => {
                handle.id = self.create_texture_view(tv.texture_id, requestor_id)?;
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
                handle.id = self.create_bind_group_layout(&entries, requestor_id);
            }
            Some(ComputeCommand::CreateBindGroup(bg)) => {
                handle.id = self.create_bind_group(bg.layout_id, &bg.entries, requestor_id)?;
            }
            _ => return Err(anyhow::anyhow!("Unsupported resource command")),
        }
        Ok(ComputeResourceResponse { handle: Some(handle), error: String::new() }.encode_to_vec())
    }

    // ── Compute: execute (record render commands) ─────────────

    pub fn execute(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        let req = Submit::decode(&payload[..])?;
        // Check write access to the target view before queueing
        if !self.registry.check_access(req.target_texture_view_id, requestor_id, Access::Write) {
            return Err(anyhow::anyhow!("Access denied to target view {}", req.target_texture_view_id));
        }

        if let Some(cb) = req.command_buffer {
            self.pending_ops.lock().unwrap().push(PendingRenderOp {
                target_view_id: req.target_texture_view_id,
                command_buffer: cb,
                instance_id: requestor_id,
            });
        }
        Ok(Vec::new())
    }

    // ── Compute: build encoder from pending op ────────────────

    pub fn build_encoder(&self, req: Submit, requestor_id: u32) -> anyhow::Result<wgpu::CommandEncoder> {
        let device = self.get_device();
        let view = match self.get_resource(req.target_texture_view_id, requestor_id) {
            Some(Resource::GpuObj(GpuObject::TextureView(v))) => v,
            _ => return Err(anyhow::anyhow!("Target view {} not found or access denied", req.target_texture_view_id)),
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
                let _ = execute_render_commands(&mut rp, cb, self, 2048, 2048, requestor_id);
            }
        }
        Ok(encoder)
    }

}

// ── Render command execution ───────────────────────────────────

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
    gpu: &'a GraphicsDevice,
    target_width: u32,
    target_height: u32,
    requestor_id: u32,
) -> anyhow::Result<()> {
    for wgpu_cmd in &command_buffer.commands {
        let cmd = match &wgpu_cmd.command { Some(c) => c, None => continue };
        match cmd {
            WgpuCommand::SetPipeline(p) => {
                if let Some(Resource::GpuObj(GpuObject::RenderPipeline(pipeline))) = gpu.get_resource(p.pipeline_id, requestor_id) {
                    rp.set_pipeline(pipeline.as_ref());
                }
            }
            WgpuCommand::SetBindGroup(bg) => {
                if let Some(Resource::GpuObj(GpuObject::BindGroup(bind_group))) = gpu.get_resource(bg.bind_group_id, requestor_id) {
                    rp.set_bind_group(bg.index, bind_group.as_ref(), &bg.dynamic_offsets);
                } else if let Some(Resource::Data(region_id)) = gpu.get_resource(bg.bind_group_id, requestor_id) {
                    if let Some((texture, _, _, _)) = gpu.get_texture_info(region_id, requestor_id) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        let bgl = get_ui_layout(&gpu.get_device());
                        let sampler = get_ui_sampler(&gpu.get_device());
                        let bg_res = gpu.get_device().create_bind_group(&wgpu::BindGroupDescriptor {
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
                if let Some(buffer) = gpu.get_buffer(vb.buffer_id, requestor_id) {
                    let end = if vb.size > 0 { (vb.offset + vb.size).min(buffer.size()) } else { buffer.size() };
                    rp.set_vertex_buffer(vb.slot, buffer.slice(vb.offset..end));
                }
            }
            WgpuCommand::SetIndexBuffer(ib) => {
                let format = if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                if let Some(buffer) = gpu.get_buffer(ib.buffer_id, requestor_id) {
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
