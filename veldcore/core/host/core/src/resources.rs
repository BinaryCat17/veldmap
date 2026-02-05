use std::sync::Arc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub enum Resource {
    Data(Vec<u8>),
    Buffer(Arc<wgpu::Buffer>),
    Texture {
        texture: Arc<wgpu::Texture>,
        width: u32,
        height: u32,
        format: u32,
    },
    TextureView(Arc<wgpu::TextureView>),
    Sampler(Arc<wgpu::Sampler>),
    BindGroupLayout(Arc<wgpu::BindGroupLayout>),
    RenderPipeline(Arc<wgpu::RenderPipeline>),
    BindGroup(Arc<wgpu::BindGroup>),
    ShaderModule(Arc<wgpu::ShaderModule>),
}

pub struct ResourceManager {
    pub resources: DashMap<u64, Resource>,
    named_resources: DashMap<String, u64>,
    pub next_id: AtomicU64,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
}

impl ResourceManager {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, surface_format: wgpu::TextureFormat) -> Self {
        Self {
            resources: DashMap::new(),
            named_resources: DashMap::new(),
            next_id: AtomicU64::new(1),
            device,
            queue,
            surface_format,
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

    pub fn get_queue(&self) -> Arc<wgpu::Queue> {
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

    pub fn create_buffer(&self, size: u64, usage: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        
        // Добавляем COPY_DST всегда, чтобы мы могли писать в буфер из WASM
        final_usage |= wgpu::BufferUsages::COPY_DST;

        if final_usage.intersects(wgpu::BufferUsages::MAP_READ) {
            // Оставляем как есть
        } else if usage == 0 {
            final_usage |= wgpu::BufferUsages::MAP_READ;
        }

        let aligned_size = Self::align_to(size, 4);

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("Buffer-{}", id)),
            size: aligned_size,
            usage: final_usage,
            mapped_at_creation: false,
        });
        self.resources.insert(id, Resource::Buffer(Arc::new(buffer)));
        id
    }

    pub fn create_texture(&self, width: u32, height: u32, format_id: u32, usage: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let format = match format_id {
            1 => wgpu::TextureFormat::R32Float,
            2 => wgpu::TextureFormat::Rgba16Float,
            3 => wgpu::TextureFormat::Rgba32Float,
            9 => wgpu::TextureFormat::R8Unorm,
            _ => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("Texture-{}", id)),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::from_bits_truncate(usage) 
                   | wgpu::TextureUsages::TEXTURE_BINDING 
                   | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.resources.insert(id, Resource::Texture { 
            texture: Arc::new(texture),
            width: width,
            height: height,
            format: format_id,
        });
        id
    }

    pub fn create_texture_view(&self, texture_id: u64) -> anyhow::Result<u64> {
        let res = self.get_resource(texture_id).ok_or_else(|| anyhow::anyhow!("Texture not found"))?;
        let view = match res {
            Resource::Texture { texture, .. } => texture.create_view(&wgpu::TextureViewDescriptor::default()),
            _ => return Err(anyhow::anyhow!("Resource is not a texture")),
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.resources.insert(id, Resource::TextureView(Arc::new(view)));
        Ok(id)
    }

    pub fn create_sampler(&self, mag: u32, min: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: if mag == 1 { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest },
            min_filter: if min == 1 { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest },
            ..Default::default()
        });
        self.resources.insert(id, Resource::Sampler(Arc::new(sampler)));
        id
    }

    pub fn create_bind_group_layout(&self, entries: &[wgpu::BindGroupLayoutEntry]) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("BGL-{}", id)),
            entries,
        });
        self.resources.insert(id, Resource::BindGroupLayout(Arc::new(layout)));
        id
    }

    pub fn create_bind_group(&self, layout_id: u64, entries_proto: &[crate::wgpu::BindGroupEntry]) -> anyhow::Result<u64> {
        let layout_res = self.get_resource(layout_id).ok_or_else(|| anyhow::anyhow!("BGL not found"))? ;
        let layout = match layout_res {
            Resource::BindGroupLayout(l) => l,
            _ => return Err(anyhow::anyhow!("Resource is not a BindGroupLayout")),
        };

        // 1. Первый проход: собираем Arc ресурсов
        let mut keep_alive_buffers = Vec::new();
        let mut keep_alive_views = Vec::new();
        let mut keep_alive_samplers = Vec::new();

        for e in entries_proto {
            match &e.resource {
                Some(crate::wgpu::bind_group_entry::Resource::BufferId(bid)) => {
                    if let Some(Resource::Buffer(b)) = self.get_resource(*bid) { keep_alive_buffers.push((e.binding, b)); }
                    else { return Err(anyhow::anyhow!("Buffer {} not found", bid)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::BufferBinding(bb)) => {
                    if let Some(Resource::Buffer(b)) = self.get_resource(bb.buffer_id) { keep_alive_buffers.push((e.binding, b)); }
                    else { return Err(anyhow::anyhow!("Buffer {} not found", bb.buffer_id)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::TextureViewId(tvid)) => {
                    if let Some(Resource::TextureView(tv)) = self.get_resource(*tvid) { keep_alive_views.push((e.binding, tv)); }
                    else if let Some(Resource::Texture { texture, .. }) = self.get_resource(*tvid) {
                         keep_alive_views.push((e.binding, Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()))));
                    }
                    else { return Err(anyhow::anyhow!("TextureView/Texture {} not found", tvid)); }
                }
                Some(crate::wgpu::bind_group_entry::Resource::SamplerId(sid)) => {
                    if let Some(Resource::Sampler(s)) = self.get_resource(*sid) { keep_alive_samplers.push((e.binding, s)); }
                    else { return Err(anyhow::anyhow!("Sampler {} not found", sid)); }
                }
                None => {}
            }
        }

        // 2. Второй проход: строим BindingResource
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
        self.resources.insert(id, Resource::BindGroup(Arc::new(bg)));
        Ok(id)
    }

    pub fn create_buffer_mapped<F>(&self, size: u64, usage: u32, fill_cb: F) -> anyhow::Result<u64> 
    where F: FnOnce(&mut [u8]) -> anyhow::Result<()> 
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        
        if final_usage.is_empty() {
            final_usage = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        }

        let aligned_size = Self::align_to(size, 4);

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("Buffer-mapped-{}", id)),
            size: aligned_size,
            usage: final_usage,
            mapped_at_creation: true,
        });
        
        {
            let mut view = buffer.slice(..).get_mapped_range_mut();
            fill_cb(&mut view[..size as usize])?;
        }
        buffer.unmap();
        
        // Гарантируем, что wgpu "увидел" размаппинг и данные доступны
        self.device.poll(wgpu::Maintain::Wait);

        self.resources.insert(id, Resource::Buffer(Arc::new(buffer)));
        Ok(id)
    }

    pub fn create_buffer_with_data(&self, data: &[u8], usage: u32) -> u64 {
        self.create_buffer_mapped(data.len() as u64, usage, |view| {
            view.copy_from_slice(data);
            Ok(())
        }).unwrap()
    }

    pub fn get_resource(&self, id: u64) -> Option<Resource> {
        self.resources.get(&id).map(|r| r.value().clone())
    }

    pub fn register_pipeline(&self, id: u64, pipeline: Arc<wgpu::RenderPipeline>) {
        self.resources.insert(id, Resource::RenderPipeline(pipeline));
    }

    pub fn register_bind_group(&self, id: u64, bind_group: Arc<wgpu::BindGroup>) {
        self.resources.insert(id, Resource::BindGroup(bind_group));
    }

    pub fn create_shader(&self, source: &str, label: Option<&str>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.resources.insert(id, Resource::ShaderModule(Arc::new(shader)));
        id
    }

    pub fn create_pipeline(&self, shader_id: u64, label: Option<&str>) -> anyhow::Result<u64> {
        let shader = match self.resources.get(&shader_id) {
            Some(r) => match r.value() {
                Resource::ShaderModule(s) => s.clone(),
                _ => return Err(anyhow::anyhow!("Resource is not a shader")),
            },
            None => return Err(anyhow::anyhow!("Shader not found")),
        };

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        
        let bind_group_layout = self.get_ui_layout();

        // Бинд-группа 1: Глобальные Uniforms (Resolution)
        let uniform_bg_layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Proxy Uniform BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Proxy Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout, &uniform_bg_layout],
            push_constant_ranges: &[],
        });

        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 32, // 2*f32 (pos) + 4*f32 (color) + 2*f32 (uv) = 8 * 4 bytes
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x2 },
                            wgpu::VertexAttribute { offset: 8, shader_location: 1, format: wgpu::VertexFormat::Float32x4 },
                            wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x2 },
                        ],
                    }
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        self.resources.insert(id, Resource::RenderPipeline(Arc::new(pipeline)));
        Ok(id)
    }

    pub fn write_resource(&self, id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        let mut resource = self.resources.get_mut(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        match resource.value_mut() {
            Resource::Data(vec) => {
                let end = (offset as usize) + data.len();
                if end > vec.len() { vec.resize(end, 0); }
                vec[offset as usize..end].copy_from_slice(data);
            },
            Resource::Buffer(buffer) => {
                self.queue.write_buffer(buffer, offset, data);
                self.device.poll(wgpu::Maintain::Poll);
            },
            Resource::Texture { texture, width, height, format } => {
                let real_block_size = match *format {
                    9 => 1,
                    2 => 8,
                    3 => 16,
                    _ => 4,
                };

                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(real_block_size * *width),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d { width: *width, height: *height, depth_or_array_layers: 1 }
                );
                self.device.poll(wgpu::Maintain::Poll);
            },
            _ => return Err(anyhow::anyhow!("Writing to this resource type is not supported")),
        }
        Ok(())
    }

    pub fn read_resource(&self, id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }
        
        let resource = self.resources.get(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        match resource.value() {
            Resource::Data(vec) => {
                let start = offset as usize;
                let end = (offset + size) as usize;
                if end > vec.len() { return Err(anyhow::anyhow!("Read out of bounds")); }
                Ok(vec[start..end].to_vec())
            },
            Resource::Buffer(buffer) => {
                let buffer = buffer.clone();
                // Пустая субмиссия для флаша всех предыдущих записей
                self.queue.submit([]);
                self.device.poll(wgpu::Maintain::Wait);

                let aligned_map_size = Self::align_to(size, 4);
                
                if buffer.usage().contains(wgpu::BufferUsages::MAP_READ) && offset + aligned_map_size <= buffer.size() {
                    let slice = buffer.slice(offset..(offset + aligned_map_size));
                    let (tx, rx) = std::sync::mpsc::channel();
                    
                    slice.map_async(wgpu::MapMode::Read, move |res| {
                        let _ = tx.send(res);
                    });
                    
                    self.device.poll(wgpu::Maintain::Wait);
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
                    self.queue.submit(Some(encoder.finish()));

                    self.device.poll(wgpu::Maintain::Wait);

                    let slice = staging.slice(..aligned_map_size);
                    let (tx, rx) = std::sync::mpsc::channel();
                    slice.map_async(wgpu::MapMode::Read, move |res| {
                        let _ = tx.send(res);
                    });
                    
                    self.device.poll(wgpu::Maintain::Wait);
                    rx.recv()??;
                    
                    let data = slice.get_mapped_range()[..size as usize].to_vec();
                    staging.unmap();
                    Ok(data)
                }
            },
            _ => Err(anyhow::anyhow!("Reading from this resource type is not supported")),
        }
    }

    pub fn create_data_resource(&self, data: Vec<u8>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.resources.insert(id, Resource::Data(data));
        id
    }

    pub fn compute_hash(&self, id: u64) -> Option<Vec<u8>> {
        let resource = self.resources.get(&id)?;
        match resource.value() {
            Resource::Data(vec) => Some(blake3::hash(vec).as_bytes().to_vec()),
            Resource::Buffer(b) => {
                let size = b.size();
                if let Ok(data) = self.read_resource(id, 0, size) {
                    return Some(blake3::hash(&data).as_bytes().to_vec());
                }
                None
            },
            _ => None,
        }
    }
}
