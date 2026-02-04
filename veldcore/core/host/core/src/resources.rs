use std::sync::Arc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub enum Resource {
    Buffer(Arc<wgpu::Buffer>),
    Texture {
        texture: Arc<wgpu::Texture>,
        width: u32,
        height: u32,
        format: u32,
    },
}

pub struct ResourceManager {
    resources: DashMap<u64, Resource>,
    next_id: AtomicU64,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl ResourceManager {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            resources: DashMap::new(),
            next_id: AtomicU64::new(1),
            device,
            queue,
        }
    }

    pub fn get_device(&self) -> Arc<wgpu::Device> {
        self.device.clone()
    }

    fn align_to(size: u64, alignment: u64) -> u64 {
        (size + alignment - 1) & !(alignment - 1)
    }

    pub fn create_buffer(&self, size: u64, usage: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        
        if final_usage.intersects(wgpu::BufferUsages::MAP_READ) {
            final_usage |= wgpu::BufferUsages::COPY_DST;
        } else if final_usage.is_empty() {
            final_usage = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        } else {
            final_usage |= wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
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
                   | wgpu::TextureUsages::COPY_DST 
                   | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.resources.insert(id, Resource::Texture { 
            texture: Arc::new(texture),
            width,
            height,
            format: format_id,
        });
        id
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

    pub fn write_resource(&self, id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        let resource = self.resources.get(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        match resource.value() {
            Resource::Buffer(buffer) => {
                self.queue.write_buffer(buffer, offset, data);
                self.device.poll(wgpu::Maintain::Poll);
            },
            Resource::Texture { texture, width, height, .. } => {
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
                        bytes_per_row: Some(4 * width),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d { width: *width, height: *height, depth_or_array_layers: 1 }
                );
                self.device.poll(wgpu::Maintain::Poll);
            },
        }
        Ok(())
    }

    pub fn read_resource(&self, id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }
        
        let resource = self.resources.get(&id).ok_or_else(|| anyhow::anyhow!("Resource not found"))?;
        let buffer = match resource.value() {
            Resource::Buffer(b) => b.clone(),
            _ => return Err(anyhow::anyhow!("Reading from textures is not supported yet")),
        };

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
    }

    pub fn compute_hash(&self, id: u64) -> Option<Vec<u8>> {
        let resource = self.resources.get(&id)?;
        let size = match resource.value() {
            Resource::Buffer(b) => b.size(),
            _ => return None,
        };
        
        if let Ok(data) = self.read_resource(id, 0, size) {
             return Some(blake3::hash(&data).as_bytes().to_vec());
        }
        None
    }
}
