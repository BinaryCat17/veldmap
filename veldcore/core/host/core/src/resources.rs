use std::sync::Arc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use crate::arena::{Arena, RegionId};
use crate::compute::{
    TextureFormat, VertexFormat, StepMode, FilterMode,
    PrimitiveTopology, BlendFactor, BlendOperation, FrontFace, CullMode,
};

// ── Unified Resource Enum ──────────────────────────────────────

#[derive(Clone)]
pub enum Resource {
    /// Data-bearing: bytes live in the Arena (CPU, GPU buffer, or GPU texture)
    Data(RegionId),
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

// ── Format helpers ─────────────────────────────────────────────

pub fn bytes_per_pixel(format_proto: i32) -> u32 {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR8Unorm => 1,
        TextureFormat::TexR32Float => 4,
        TextureFormat::TexRgba16Float => 8,
        TextureFormat::TexRgba32Float => 16,
        _ => 4,
    }
}

pub fn proto_to_wgpu_format(format_proto: i32) -> wgpu::TextureFormat {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
        TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
        TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
        TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        _ => wgpu::TextureFormat::Rgba8Unorm,
    }
}

pub fn surface_format_to_proto(fmt: wgpu::TextureFormat) -> i32 {
    match fmt {
        wgpu::TextureFormat::R32Float => TextureFormat::TexR32Float as i32,
        wgpu::TextureFormat::Rgba16Float => TextureFormat::TexRgba16Float as i32,
        wgpu::TextureFormat::Rgba32Float => TextureFormat::TexRgba32Float as i32,
        wgpu::TextureFormat::R8Unorm => TextureFormat::TexR8Unorm as i32,
        wgpu::TextureFormat::Bgra8UnormSrgb => TextureFormat::TexBgra8UnormSrgb as i32,
        wgpu::TextureFormat::Rgba8UnormSrgb => TextureFormat::TexRgba8UnormSrgb as i32,
        _ => TextureFormat::TexRgba8Unorm as i32,
    }
}

// ── Resource Manager ───────────────────────────────────────────

/// Thin ID tracker for GPU objects (non-data resources)
struct GpuEntry {
    obj: GpuObject,
    owner_id: u32,
}

pub struct ResourceManager {
    arena: Arc<Arena>,
    gpu_objects: DashMap<u64, GpuEntry>,
    named_resources: DashMap<String, u64>,
    next_gpu_id: AtomicU64,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
    surface_format: wgpu::TextureFormat,
}

impl ResourceManager {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<std::sync::Mutex<wgpu::Queue>>, surface_format: wgpu::TextureFormat) -> Self {
        let arena = Arc::new(Arena::new(device.clone(), queue.clone()));
        Self {
            arena,
            gpu_objects: DashMap::new(),
            named_resources: DashMap::new(),
            next_gpu_id: AtomicU64::new(1_000_000), // GPU objects start at 1M to avoid collisions
            device,
            queue,
            surface_format,
        }
    }

    pub fn arena(&self) -> &Arc<Arena> { &self.arena }
    pub fn get_device(&self) -> Arc<wgpu::Device> { self.device.clone() }
    pub fn get_queue(&self) -> Arc<std::sync::Mutex<wgpu::Queue>> { self.queue.clone() }
    pub fn get_surface_format_proto(&self) -> i32 { surface_format_to_proto(self.surface_format) }

    // ── Named resources ──────────────────────────────────────

    pub fn register_named_resource(&self, name: &str, id: u64) {
        self.named_resources.insert(name.to_string(), id);
    }

    pub fn get_named_resource(&self, name: &str) -> Option<u64> {
        self.named_resources.get(name).map(|r| *r.value())
    }

    // ── GPU object helpers ────────────────────────────────────

    fn insert_gpu(&self, obj: GpuObject, owner_id: u32) -> u64 {
        let id = self.next_gpu_id.fetch_add(1, Ordering::SeqCst);
        self.gpu_objects.insert(id, GpuEntry { obj, owner_id });
        id
    }

    fn get_gpu(&self, id: u64) -> Option<GpuObject> {
        self.gpu_objects.get(&id).map(|e| e.obj.clone())
    }

    // ── Unified lookup ────────────────────────────────────────

    pub fn get_resource(&self, id: u64, _requestor_id: u32) -> Option<Resource> {
        if let Some(entry) = self.gpu_objects.get(&id) {
            return Some(Resource::GpuObj(entry.obj.clone()));
        }
        if self.arena.exists(id) {
            return Some(Resource::Data(id));
        }
        None
    }

    /// Extract a wgpu::Buffer from a resource ID (Data + Buffer/Mapped backing)
    pub fn get_buffer(&self, id: u64) -> Option<Arc<wgpu::Buffer>> {
        self.arena.get_buffer(id)
    }

    /// Extract a wgpu::Texture from a resource ID (Data + Texture backing)
    pub fn get_texture(&self, id: u64) -> Option<Arc<wgpu::Texture>> {
        self.arena.get_texture(id).map(|(t, _, _, _)| t)
    }

    /// Extract texture info from a resource ID
    pub fn get_texture_info(&self, id: u64) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        self.arena.get_texture(id)
    }

    // ── GPU object creation ───────────────────────────────────

    pub fn create_texture_view(&self, texture_id: u64, owner_id: u32) -> anyhow::Result<u64> {
        let (texture, _, _, _) = self.arena.get_texture(texture_id)
            .ok_or_else(|| anyhow::anyhow!("Texture region {} not found", texture_id))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(self.insert_gpu(GpuObject::TextureView(Arc::new(view)), owner_id))
    }

    pub fn create_sampler(&self, mag_proto: i32, min_proto: i32, owner_id: u32) -> u64 {
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
        self.insert_gpu(GpuObject::Sampler(Arc::new(sampler)), owner_id)
    }

    pub fn create_bind_group_layout(&self, entries: &[wgpu::BindGroupLayoutEntry], owner_id: u32) -> u64 {
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries,
        });
        self.insert_gpu(GpuObject::BindGroupLayout(Arc::new(layout)), owner_id)
    }

    pub fn create_shader(&self, source: &str, label: Option<&str>, owner_id: u32) -> u64 {
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label, source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.insert_gpu(GpuObject::ShaderModule(Arc::new(shader)), owner_id)
    }

    pub fn create_bind_group(&self, layout_id: u64, entries_proto: &[crate::compute::BindGroupEntry], owner_id: u32) -> anyhow::Result<u64> {
        let layout = match self.get_gpu(layout_id) {
            Some(GpuObject::BindGroupLayout(l)) => l,
            _ => return Err(anyhow::anyhow!("BGL {} not found", layout_id)),
        };

        let mut keep_buffers: Vec<(u32, Arc<wgpu::Buffer>)> = Vec::new();
        let mut keep_views: Vec<(u32, Arc<wgpu::TextureView>)> = Vec::new();
        let mut keep_samplers: Vec<(u32, Arc<wgpu::Sampler>)> = Vec::new();

        for e in entries_proto {
            match &e.resource {
                Some(crate::compute::bind_group_entry::Resource::BufferId(bid)) => {
                    let b = self.arena.get_buffer(*bid)
                        .ok_or_else(|| anyhow::anyhow!("Buffer {} not found", bid))?;
                    keep_buffers.push((e.binding, b));
                }
                Some(crate::compute::bind_group_entry::Resource::BufferBinding(bb)) => {
                    let b = self.arena.get_buffer(bb.buffer_id)
                        .ok_or_else(|| anyhow::anyhow!("Buffer {} not found", bb.buffer_id))?;
                    keep_buffers.push((e.binding, b));
                }
                Some(crate::compute::bind_group_entry::Resource::TextureViewId(tvid)) => {
                    match self.get_gpu(*tvid) {
                        Some(GpuObject::TextureView(tv)) => { keep_views.push((e.binding, tv)); }
                        _ => {
                            // Fallback: try as texture region → create view on the fly
                            if let Some((tex, _, _, _)) = self.arena.get_texture(*tvid) {
                                let v = Arc::new(tex.create_view(&wgpu::TextureViewDescriptor::default()));
                                keep_views.push((e.binding, v));
                            } else {
                                return Err(anyhow::anyhow!("TextureView {} not found", tvid));
                            }
                        }
                    }
                }
                Some(crate::compute::bind_group_entry::Resource::SamplerId(sid)) => {
                    let s = match self.get_gpu(*sid) {
                        Some(GpuObject::Sampler(s)) => s,
                        _ => return Err(anyhow::anyhow!("Sampler {} not found", sid)),
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
        Ok(self.insert_gpu(GpuObject::BindGroup(Arc::new(bg)), owner_id))
    }

    pub fn create_pipeline(&self, req: &crate::compute::CreateRenderPipeline, owner_id: u32) -> anyhow::Result<u64> {
        let shader = match self.get_gpu(req.shader_id) {
            Some(GpuObject::ShaderModule(s)) => s,
            _ => return Err(anyhow::anyhow!("Shader {} not found", req.shader_id)),
        };

        let target_format = proto_to_wgpu_format(req.target_format);

        let mut bgl_refs = Vec::new();
        for &id in &req.bind_group_layout_ids {
            match self.get_gpu(id) {
                Some(GpuObject::BindGroupLayout(l)) => bgl_refs.push(l),
                _ => return Err(anyhow::anyhow!("BGL {} not found", id)),
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

        Ok(self.insert_gpu(GpuObject::RenderPipeline(Arc::new(pipeline)), owner_id))
    }

    // ── Lifecycle ─────────────────────────────────────────────

    pub fn destroy_resource(&self, id: u64, requestor_id: u32) {
        if self.arena.free(id, requestor_id) { return; }
        // Try GPU objects
        let can_remove = self.gpu_objects.get(&id)
            .map(|e| e.owner_id == requestor_id || requestor_id == 0)
            .unwrap_or(false);
        if can_remove { self.gpu_objects.remove(&id); }
    }

    pub fn cleanup_resources(&self, owner_id: u32) {
        if owner_id == 0 { return; }
        self.arena.cleanup_owner(owner_id);
        self.gpu_objects.retain(|_, e| e.owner_id != owner_id);
    }

    pub fn compute_hash(&self, id: u64, requestor_id: u32) -> Option<Vec<u8>> {
        self.arena.compute_hash(id, requestor_id)
    }

}
