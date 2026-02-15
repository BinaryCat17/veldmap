use std::sync::Arc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, AtomicI32, Ordering};
use crate::wgpu::{
    TextureFormat, VertexFormat, StepMode, FilterMode, 
    PrimitiveTopology, BlendFactor, BlendOperation, FrontFace, CullMode, IndexFormat
};

#[derive(Clone, Debug)]
pub enum Resource {
    Data(Vec<u8>),
    Buffer(Arc<wgpu::Buffer>),
    Texture {
        texture: Arc<wgpu::Texture>,
        width: u32,
        height: u32,
        format: i32,
    },
    TextureView(Arc<wgpu::TextureView>),
    Sampler(Arc<wgpu::Sampler>),
    BindGroupLayout(Arc<wgpu::BindGroupLayout>),
    RenderPipeline(Arc<wgpu::RenderPipeline>),
    BindGroup(Arc<wgpu::BindGroup>),
    ShaderModule(Arc<wgpu::ShaderModule>),
}

#[derive(Debug)]
pub struct ResourceEntry {
    pub resource: Resource,
    pub readonly: bool,
    pub ref_count: AtomicI32,
    pub owner_id: u32, // 0 for system resources
}

impl Clone for ResourceEntry {
    fn clone(&self) -> Self {
        Self {
            resource: self.resource.clone(),
            readonly: self.readonly,
            ref_count: AtomicI32::new(self.ref_count.load(Ordering::SeqCst)),
            owner_id: self.owner_id,
        }
    }
}

pub struct ResourceManager {
    pub resources: DashMap<u64, ResourceEntry>,
    named_resources: DashMap<String, u64>,
    pub next_id: AtomicU64,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
    surface_format: wgpu::TextureFormat,
}

impl ResourceManager {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<std::sync::Mutex<wgpu::Queue>>, surface_format: wgpu::TextureFormat) -> Self {
        Self {
            resources: DashMap::new(),
            named_resources: DashMap::new(),
            next_id: AtomicU64::new(1),
            device,
            queue,
            surface_format,
        }
    }

    pub fn get_surface_format_proto(&self) -> i32 {
        match self.surface_format {
            wgpu::TextureFormat::R32Float => TextureFormat::TexR32Float as i32,
            wgpu::TextureFormat::Rgba16Float => TextureFormat::TexRgba16Float as i32,
            wgpu::TextureFormat::Rgba32Float => TextureFormat::TexRgba32Float as i32,
            wgpu::TextureFormat::R8Unorm => TextureFormat::TexR8Unorm as i32,
            wgpu::TextureFormat::Bgra8UnormSrgb => TextureFormat::TexBgra8UnormSrgb as i32,
            wgpu::TextureFormat::Rgba8UnormSrgb => TextureFormat::TexRgba8UnormSrgb as i32,
            _ => TextureFormat::TexRgba8Unorm as i32,
        }
    }

    pub fn register_named_resource(&self, name: &str, id: u64) {
        self.named_resources.insert(name.to_string(), id);
    }

    pub fn get_named_resource(&self, name: &str) -> Option<u64> {
        self.named_resources.get(name).map(|r| *r.value())
    }

    pub fn get_device(&self) -> Arc<wgpu::Device> {
        self.device.clone()
    }

    pub fn get_queue(&self) -> Arc<std::sync::Mutex<wgpu::Queue>> {
        self.queue.clone()
    }

    pub fn get_ui_layout(&self) -> wgpu::BindGroupLayout {
        self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

    pub fn get_ui_sampler(&self) -> wgpu::Sampler {
        self.device.create_sampler(&wgpu::SamplerDescriptor { 
            address_mode_u: wgpu::AddressMode::ClampToEdge, 
            address_mode_v: wgpu::AddressMode::ClampToEdge, 
            mag_filter: wgpu::FilterMode::Linear, 
            min_filter: wgpu::FilterMode::Linear, 
            ..Default::default() 
        })
    }

    fn align_to(size: u64, alignment: u64) -> u64 {
        (size + alignment - 1) & !(alignment - 1)
    }

    fn insert_resource(&self, id: u64, resource: Resource, readonly: bool, owner_id: u32) {
        self.resources.insert(id, ResourceEntry {
            resource,
            readonly,
            ref_count: AtomicI32::new(1),
            owner_id,
        });
    }

    pub fn acquire_resource(&self, id: u64, requestor_id: u32) -> bool {
        if let Some(entry) = self.resources.get(&id) {
            // Allow access if owner or system resource
            if entry.owner_id != 0 && entry.owner_id != requestor_id {
                return false;
            }
            entry.ref_count.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn release_resource(&self, id: u64, requestor_id: u32) {
        let mut should_remove = false;
        if let Some(entry) = self.resources.get(&id) {
            if entry.owner_id != 0 && entry.owner_id != requestor_id {
                log::warn!(target: "host", "Unauthorized release attempt for resource {} by instance {}", id, requestor_id);
                return;
            }
            let prev = entry.ref_count.fetch_sub(1, Ordering::SeqCst);
            if prev <= 1 {
                should_remove = true;
            }
        }
        if should_remove {
            log::debug!(target: "host", "Resource {} ref_count dropped to 0, removing.", id);
            self.resources.remove(&id);
        }
    }

    pub fn cleanup_resources(&self, owner_id: u32) {
        if owner_id == 0 { return; }
        log::info!(target: "host", "Cleaning up resources for instance {}", owner_id);
        self.resources.retain(|_, entry| entry.owner_id != owner_id);
    }

    pub fn destroy_resource(&self, id: u64, requestor_id: u32) {
        if let Some(entry) = self.resources.get(&id) {
            if entry.owner_id != 0 && entry.owner_id != requestor_id {
                return;
            }
        }
        self.resources.remove(&id);
    }

    pub fn create_buffer_ext(&self, size: u64, usage: u32, mapped: bool, readonly: bool, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !readonly || !mapped { final_usage |= wgpu::BufferUsages::COPY_DST; }
        let aligned_size = Self::align_to(size, 4);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("Buffer-{}", id)),
            size: aligned_size,
            usage: final_usage,
            mapped_at_creation: mapped,
        });
        self.insert_resource(id, Resource::Buffer(Arc::new(buffer)), readonly, owner_id);
        id
    }

    pub fn create_buffer(&self, size: u64, usage: u32, owner_id: u32) -> u64 {
        self.create_buffer_ext(size, usage, false, false, owner_id)
    }

    pub fn create_texture(&self, width: u32, height: u32, format_proto: i32, usage: u32, readonly: bool, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let format = match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
            TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
            TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
            TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
            TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
            TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            _ => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("Texture-{}", id)),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::from_bits_truncate(usage) | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.insert_resource(id, Resource::Texture { texture: Arc::new(texture), width, height, format: format_proto }, readonly, owner_id);
        id
    }

    pub fn create_texture_view(&self, texture_id: u64, owner_id: u32) -> anyhow::Result<u64> {
        let entry = self.resources.get(&texture_id).ok_or_else(|| anyhow::anyhow!("Texture not found"))?;
        // View creation is allowed for sharing
        
        let view = match &entry.resource {
            Resource::Texture { texture, .. } => texture.create_view(&wgpu::TextureViewDescriptor::default()),
            _ => return Err(anyhow::anyhow!("Resource is not a texture")),
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.insert_resource(id, Resource::TextureView(Arc::new(view)), entry.readonly, owner_id);
        Ok(id)
    }

    pub fn create_sampler(&self, mag_proto: i32, min_proto: i32, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mag = match FilterMode::try_from(mag_proto).unwrap_or(FilterMode::FiltLinear) {
            FilterMode::FiltNearest => wgpu::FilterMode::Nearest,
            FilterMode::FiltLinear => wgpu::FilterMode::Linear,
        };
        let min = match FilterMode::try_from(min_proto).unwrap_or(FilterMode::FiltLinear) {
            FilterMode::FiltNearest => wgpu::FilterMode::Nearest,
            FilterMode::FiltLinear => wgpu::FilterMode::Linear,
        };
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: mag,
            min_filter: min,
            ..Default::default()
        });
        self.insert_resource(id, Resource::Sampler(Arc::new(sampler)), true, owner_id);
        id
    }

    pub fn create_bind_group_layout(&self, entries: &[wgpu::BindGroupLayoutEntry], owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("BGL-{}", id)),
            entries,
        });
        self.insert_resource(id, Resource::BindGroupLayout(Arc::new(layout)), true, owner_id);
        id
    }

    pub fn create_bind_group(&self, layout_id: u64, entries_proto: &[crate::wgpu::BindGroupEntry], owner_id: u32) -> anyhow::Result<u64> {
        let layout_res = self.get_resource(layout_id, owner_id).ok_or_else(|| anyhow::anyhow!("BGL not found"))? ;
        let layout = match layout_res {
            Resource::BindGroupLayout(l) => l,
            _ => return Err(anyhow::anyhow!("Resource is not a BindGroupLayout")),
        };

        let mut keep_alive_buffers = Vec::new();
        let mut keep_alive_views = Vec::new();
        let mut keep_alive_samplers = Vec::new();

        for e in entries_proto {
            match &e.resource {
                Some(crate::wgpu::bind_group_entry::Resource::BufferId(bid)) => {
                    if let Some(Resource::Buffer(b)) = self.get_resource(*bid, owner_id) { keep_alive_buffers.push((e.binding, b)); }
                    else { return Err(anyhow::anyhow!("Buffer {} not found or unauthorized", bid)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::BufferBinding(bb)) => {
                    if let Some(Resource::Buffer(b)) = self.get_resource(bb.buffer_id, owner_id) { keep_alive_buffers.push((e.binding, b)); }
                    else { return Err(anyhow::anyhow!("Buffer {} not found or unauthorized", bb.buffer_id)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::TextureViewId(tvid)) => {
                    if let Some(Resource::TextureView(tv)) = self.get_resource(*tvid, owner_id) { keep_alive_views.push((e.binding, tv)); }
                    else if let Some(Resource::Texture { texture, .. }) = self.get_resource(*tvid, owner_id) {
                         keep_alive_views.push((e.binding, Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()))));
                    }
                    else { return Err(anyhow::anyhow!("TextureView/Texture {} not found or unauthorized", tvid)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::SamplerId(sid)) => {
                    if let Some(Resource::Sampler(s)) = self.get_resource(*sid, owner_id) { keep_alive_samplers.push((e.binding, s)); }
                    else { return Err(anyhow::anyhow!("Sampler {} not found or unauthorized", sid)); }
                }
                None => {}
            }
        }

        let mut entries = Vec::new();
        for e in entries_proto {
            let resource = match &e.resource {
                Some(crate::wgpu::bind_group_entry::Resource::BufferId(_)) => {
                    let b = &keep_alive_buffers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Buffer(b.as_entire_buffer_binding())
                }
                Some(crate::wgpu::bind_group_entry::Resource::BufferBinding(bb)) => {
                    let b = &keep_alive_buffers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: b,
                        offset: bb.offset,
                        size: std::num::NonZeroU64::new(bb.size),
                    })
                }
                Some(crate::wgpu::bind_group_entry::Resource::TextureViewId(_)) => {
                    let tv = &keep_alive_views.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::TextureView(tv)
                }
                Some(crate::wgpu::bind_group_entry::Resource::SamplerId(_)) => {
                    let s = &keep_alive_samplers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Sampler(s)
                }
                None => return Err(anyhow::anyhow!("BindGroup entry resource missing")),
            };
            entries.push(wgpu::BindGroupEntry {
                binding: e.binding,
                resource,
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("BG-{}", id)),
            layout: &layout,
            entries: &entries,
        });
        self.insert_resource(id, Resource::BindGroup(Arc::new(bg)), true, owner_id);
        Ok(id)
    }

    pub fn create_buffer_with_data(&self, data: &[u8], usage: u32, readonly: bool, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !readonly { final_usage |= wgpu::BufferUsages::COPY_DST; }
        let aligned_size = Self::align_to(data.len() as u64, 4);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("Buffer-data-{}", id)),
            size: aligned_size,
            usage: final_usage,
            mapped_at_creation: true,
        });
        {
            let mut view = buffer.slice(..).get_mapped_range_mut();
            view[..data.len()].copy_from_slice(data);
        }
        buffer.unmap();
        self.insert_resource(id, Resource::Buffer(Arc::new(buffer)), readonly, owner_id);
        id
    }

    pub fn freeze_resource(&self, id: u64, requestor_id: u32) -> bool {
        if let Some(mut entry) = self.resources.get_mut(&id) {
            if entry.owner_id != 0 && entry.owner_id != requestor_id {
                return false;
            }
            entry.readonly = true;
            true
        } else {
            false
        }
    }

    pub fn get_resource(&self, id: u64, _requestor_id: u32) -> Option<Resource> {
        self.resources.get(&id).map(|r| r.value().resource.clone())
    }

    pub fn get_resource_entry(&self, id: u64, _requestor_id: u32) -> Option<ResourceEntry> {
        self.resources.get(&id).map(|r| r.value().clone())
    }

    pub fn register_pipeline(&self, id: u64, pipeline: Arc<wgpu::RenderPipeline>, owner_id: u32) {
        self.insert_resource(id, Resource::RenderPipeline(pipeline), true, owner_id);
    }

    pub fn register_bind_group(&self, id: u64, bind_group: Arc<wgpu::BindGroup>, owner_id: u32) {
        self.insert_resource(id, Resource::BindGroup(bind_group), true, owner_id);
    }

    pub fn create_shader(&self, source: &str, label: Option<&str>, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.insert_resource(id, Resource::ShaderModule(Arc::new(shader)), true, owner_id);
        id
    }

    pub fn create_pipeline(&self, req: &crate::wgpu::CreateRenderPipeline, owner_id: u32) -> anyhow::Result<u64> {
        let shader = match self.get_resource(req.shader_id, owner_id) {
            Some(Resource::ShaderModule(s)) => s,
            _ => return Err(anyhow::anyhow!("Resource is not a shader or unauthorized")),
        };

        let target_format = match TextureFormat::try_from(req.target_format).unwrap_or(TextureFormat::TexRgba8Unorm) {
            TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
            TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
            TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
            TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
            TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            _ => wgpu::TextureFormat::Rgba8Unorm,
        };

        let mut bgls = Vec::new();
        let mut bgl_refs = Vec::new();
        for &id in &req.bind_group_layout_ids {
            match self.get_resource(id, owner_id) {
                Some(Resource::BindGroupLayout(l)) => {
                    bgls.push(l);
                }
                _ => return Err(anyhow::anyhow!("BindGroupLayout {} not found or unauthorized", id)),
            }
        }
        for l in &bgls { bgl_refs.push(l.as_ref()); }

        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Proxy Pipeline Layout"),
            bind_group_layouts: &bgl_refs,
            immediate_size: 0,
        });

        let mut wgpu_vertex_layouts = Vec::new();
        let mut keep_alive_attributes = Vec::new();

        for vl in &req.vertex_layouts {
            let mut attrs = Vec::new();
            for attr in &vl.attributes {
                attrs.push(wgpu::VertexAttribute {
                    offset: attr.offset,
                    shader_location: attr.shader_location,
                    format: match VertexFormat::try_from(attr.format).unwrap_or(VertexFormat::VtxFloat32x2) {
                        VertexFormat::VtxFloat32 => wgpu::VertexFormat::Float32,
                        VertexFormat::VtxFloat32x2 => wgpu::VertexFormat::Float32x2,
                        VertexFormat::VtxFloat32x3 => wgpu::VertexFormat::Float32x3,
                        VertexFormat::VtxFloat32x4 => wgpu::VertexFormat::Float32x4,
                        VertexFormat::VtxUint32 => wgpu::VertexFormat::Uint32,
                    },
                });
            }
            keep_alive_attributes.push(attrs);
        }
        
        for i in 0..req.vertex_layouts.len() {
            wgpu_vertex_layouts.push(wgpu::VertexBufferLayout {
                array_stride: req.vertex_layouts[i].array_stride,
                step_mode: if StepMode::try_from(req.vertex_layouts[i].step_mode).unwrap_or(StepMode::StepVertex) == StepMode::StepInstance { 
                    wgpu::VertexStepMode::Instance 
                } else { 
                    wgpu::VertexStepMode::Vertex 
                },
                attributes: &keep_alive_attributes[i],
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

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&req.label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(&req.vertex_entry),
                buffers: &wgpu_vertex_layouts,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(&req.fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
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
                                   || req.primitive_topology == PrimitiveTopology::TopologyLineStrip as i32 {
                    match IndexFormat::try_from(req.strip_index_format).unwrap_or(IndexFormat::IdxUint16) {
                        IndexFormat::IdxUint16 => Some(wgpu::IndexFormat::Uint16),
                        IndexFormat::IdxUint32 => Some(wgpu::IndexFormat::Uint32),
                    }
                } else {
                    None
                },
                front_face: match FrontFace::try_from(req.front_face).unwrap_or(FrontFace::Ccw) {
                    FrontFace::Ccw => wgpu::FrontFace::Ccw,
                    FrontFace::Cw => wgpu::FrontFace::Cw,
                },
                cull_mode: match CullMode::try_from(req.cull_mode).unwrap_or(CullMode::None) {
                    CullMode::None => None,
                    CullMode::Front => Some(wgpu::Face::Front),
                    CullMode::Back => Some(wgpu::Face::Back),
                },
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None, // TODO: Support depth stencil
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        self.insert_resource(id, Resource::RenderPipeline(Arc::new(pipeline)), true, owner_id);
        Ok(id)
    }

    pub fn write_resource(&self, id: u64, offset: u64, data: &[u8], requestor_id: u32) -> anyhow::Result<()> {
        let mut resource_entry = self.resources.get_mut(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        if requestor_id != 0 && resource_entry.owner_id != 0 && resource_entry.owner_id != requestor_id {
            return Err(anyhow::anyhow!("Unauthorized write access to resource {}", id));
        }
        if resource_entry.readonly {
            return Err(anyhow::anyhow!("Resource {} is readonly", id));
        }

        match resource_entry.value_mut().resource {
            Resource::Data(ref mut vec) => {
                let end = (offset as usize) + data.len();
                if end > vec.len() { vec.resize(end, 0); }
                vec[offset as usize..end].copy_from_slice(data);
            },
            Resource::Buffer(ref buffer) => {
                let q = self.queue.lock().unwrap();
                q.write_buffer(buffer, offset, data);
            },
            Resource::Texture { ref texture, width, height, format } => {
                let bytes_per_pixel = match TextureFormat::try_from(format).unwrap_or(TextureFormat::TexRgba8Unorm) {
                    TextureFormat::TexR8Unorm => 1,
                    TextureFormat::TexR32Float => 4,
                    TextureFormat::TexRgba16Float => 8,
                    TextureFormat::TexRgba32Float => 16,
                    _ => 4,
                };

                let bytes_per_row = bytes_per_pixel * width;
                let y_origin = (offset / bytes_per_row as u64) as u32;
                let rows_to_write = (data.len() as u32 + bytes_per_row - 1) / bytes_per_row;
                let height_to_write = rows_to_write.min(height - y_origin);

                let q = self.queue.lock().unwrap();
                q.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: 0, y: y_origin, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(height_to_write),
                    },
                    wgpu::Extent3d { width, height: height_to_write, depth_or_array_layers: 1 }
                );
            },
            _ => return Err(anyhow::anyhow!("Writing to this resource type is not supported")),
        }
        Ok(())
    }

    pub fn read_resource(&self, id: u64, offset: u64, size: u64, _requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }
        
        let entry = self.resources.get(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        // Read access is allowed for everyone who has the ID
        
        match &entry.resource {
            Resource::Data(vec) => {
                let start = offset as usize;
                let end = (offset + size) as usize;
                if end > vec.len() { return Err(anyhow::anyhow!("Read out of bounds")); }
                Ok(vec[start..end].to_vec())
            },
            Resource::Buffer(buffer) => {
                let buffer = buffer.clone();
                {
                    let q = self.queue.lock().unwrap();
                    q.submit([]);
                    let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                }

                let aligned_map_size = Self::align_to(size, 4);
                if buffer.usage().contains(wgpu::BufferUsages::MAP_READ) && offset + aligned_map_size <= buffer.size() {
                    let slice = buffer.slice(offset..(offset + aligned_map_size));
                    let (tx, rx) = std::sync::mpsc::channel();
                    slice.map_async(wgpu::MapMode::Read, move |res| { let _ = tx.send(res); });
                    {
                        let _q = self.queue.lock().unwrap();
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    rx.recv()??;
                    let data = slice.get_mapped_range()[..size as usize].to_vec();
                    buffer.unmap();
                    Ok(data)
                } else {
                    let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Staging-Read"),
                        size: aligned_map_size,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    encoder.copy_buffer_to_buffer(&buffer, offset, &staging, 0, size);
                    {
                        let q = self.queue.lock().unwrap();
                        q.submit(Some(encoder.finish()));
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    let slice = staging.slice(..aligned_map_size);
                    let (tx, rx) = std::sync::mpsc::channel();
                    slice.map_async(wgpu::MapMode::Read, move |res| { let _ = tx.send(res); });
                    {
                        let _q = self.queue.lock().unwrap();
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    rx.recv()??;
                    let data = slice.get_mapped_range()[..size as usize].to_vec();
                    staging.unmap();
                    Ok(data)
                }
            },
            _ => Err(anyhow::anyhow!("Reading from this resource type is not supported")),
        }
    }

    pub fn create_data_resource(&self, data: Vec<u8>, owner_id: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.insert_resource(id, Resource::Data(data), false, owner_id);
        id
    }

    pub fn compute_hash(&self, id: u64, requestor_id: u32) -> Option<Vec<u8>> {
        let entry = self.resources.get(&id)?;
        if entry.owner_id != 0 && entry.owner_id != requestor_id {
            return None;
        }
        match &entry.resource {
            Resource::Data(vec) => Some(blake3::hash(vec).as_bytes().to_vec()),
            Resource::Buffer(b) => {
                let size = b.size();
                if let Ok(data) = self.read_resource(id, 0, size, requestor_id) {
                    return Some(blake3::hash(&data).as_bytes().to_vec());
                }
                None
            },
            _ => None,
        }
    }
}
