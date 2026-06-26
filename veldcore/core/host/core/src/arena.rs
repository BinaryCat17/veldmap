use std::sync::Arc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use crate::compute::TextureFormat;

pub type RegionId = u64;

// ── Format helpers (self-contained) ────────────────────────────

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

/// How the bytes in a region are backed
pub enum DataBacking {
    Cpu(Vec<u8>),
    Buffer(Arc<wgpu::Buffer>),
    Mapped(Arc<wgpu::Buffer>),
    Texture {
        texture: Arc<wgpu::Texture>,
        width: u32,
        height: u32,
        format: i32,
    },
}

impl DataBacking {
    pub fn is_buffer(&self) -> bool {
        matches!(self, Self::Buffer(_) | Self::Mapped(_))
    }

    pub fn as_buffer(&self) -> Option<&wgpu::Buffer> {
        match self {
            Self::Buffer(b) | Self::Mapped(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_texture(&self) -> Option<&wgpu::Texture> {
        match self {
            Self::Texture { texture, .. } => Some(texture),
            _ => None,
        }
    }

    pub fn byte_len(&self) -> u64 {
        match self {
            Self::Cpu(v) => v.len() as u64,
            Self::Buffer(b) | Self::Mapped(b) => b.size(),
            Self::Texture { width, height, format, .. } => {
                let bpp = bytes_per_pixel(*format);
                (*width as u64) * (*height as u64) * (bpp as u64)
            }
        }
    }
}

/// A data-bearing region in the arena
pub struct Region {
    pub backing: DataBacking,
    pub readonly: bool,
    pub owner_id: u32,
}

/// Access lease for a region
pub struct Lease {
    pub owner_id: u32,
    pub readers: Vec<u32>,
    pub expires_at: Option<Instant>,
}

impl Lease {
    pub fn new(owner_id: u32) -> Self {
        Self { owner_id, readers: Vec::new(), expires_at: None }
    }

    pub fn can_read(&self, module_id: u32) -> bool {
        self.owner_id == module_id
            || module_id == 0
            || self.readers.contains(&module_id)
    }

    pub fn can_write(&self, module_id: u32) -> bool {
        (self.owner_id == module_id || module_id == 0)
            && self.expires_at.map_or(true, |e| e > Instant::now())
    }

    pub fn add_reader(&mut self, module_id: u32) {
        if !self.readers.contains(&module_id) && module_id != self.owner_id {
            self.readers.push(module_id);
        }
    }

    pub fn remove_reader(&mut self, module_id: u32) {
        self.readers.retain(|&r| r != module_id);
    }

    pub fn revoke_all(&mut self) {
        self.readers.clear();
        self.expires_at = Some(Instant::now());
    }
}

/// Host-managed shared arena: allocator + lease table
pub struct Arena {
    regions: DashMap<RegionId, Region>,
    leases: DashMap<RegionId, Lease>,
    next_id: AtomicU64,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
}

impl Arena {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<std::sync::Mutex<wgpu::Queue>>) -> Self {
        Self {
            regions: DashMap::new(),
            leases: DashMap::new(),
            next_id: AtomicU64::new(1),
            device,
            queue,
        }
    }

    fn next_region_id(&self) -> RegionId {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    // ── Allocation ────────────────────────────────────────────

    pub fn alloc(&self, backing: DataBacking, readonly: bool, owner_id: u32) -> RegionId {
        let id = self.next_region_id();
        self.regions.insert(id, Region { backing, readonly, owner_id });
        self.leases.insert(id, Lease::new(owner_id));
        id
    }

    pub fn alloc_cpu(&self, data: Vec<u8>, owner_id: u32) -> RegionId {
        self.alloc(DataBacking::Cpu(data), false, owner_id)
    }

    pub fn alloc_buffer(&self, size: u64, usage: u32, mapped: bool, readonly: bool, owner_id: u32) -> RegionId {
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !mapped { final_usage |= wgpu::BufferUsages::COPY_DST; }
        if mapped { final_usage |= wgpu::BufferUsages::MAP_WRITE; }
        let aligned = (size + 3) & !3;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("arena-buf-{}", self.next_id.load(Ordering::Relaxed))),
            size: aligned,
            usage: final_usage,
            mapped_at_creation: mapped,
        });
        let backing = if mapped {
            DataBacking::Mapped(Arc::new(buffer))
        } else {
            DataBacking::Buffer(Arc::new(buffer))
        };
        self.alloc(backing, readonly, owner_id)
    }

    pub fn alloc_texture(&self, width: u32, height: u32, format_proto: i32, usage: u32, readonly: bool, owner_id: u32) -> RegionId {
        let format = proto_to_wgpu_format(format_proto);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("arena-tex-{}", self.next_id.load(Ordering::Relaxed))),
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
        self.alloc(
            DataBacking::Texture { texture: Arc::new(texture), width, height, format: format_proto },
            readonly,
            owner_id,
        )
    }

    pub fn alloc_buffer_with_data(&self, data: &[u8], usage: u32, readonly: bool, owner_id: u32) -> RegionId {
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !readonly { final_usage |= wgpu::BufferUsages::COPY_DST; }
        let aligned = ((data.len() as u64) + 3) & !3;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("arena-buf-data-{}", self.next_id.load(Ordering::Relaxed))),
            size: aligned,
            usage: final_usage,
            mapped_at_creation: true,
        });
        {
            let mut view = buffer.slice(..).get_mapped_range_mut();
            view[..data.len()].copy_from_slice(data);
        }
        buffer.unmap();
        self.alloc(DataBacking::Buffer(Arc::new(buffer)), readonly, owner_id)
    }

    // ── Lease management ──────────────────────────────────────

    pub fn grant_read(&self, region_id: RegionId, target_module: u32, owner_id: u32) -> bool {
        if let Some(mut lease) = self.leases.get_mut(&region_id) {
            if lease.can_write(owner_id) {
                lease.add_reader(target_module);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn revoke_access(&self, region_id: RegionId, owner_id: u32) -> bool {
        if let Some(mut lease) = self.leases.get_mut(&region_id) {
            if lease.owner_id == owner_id || owner_id == 0 {
                lease.revoke_all();
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn transfer(&self, region_id: RegionId, new_owner: u32, current_owner: u32) -> bool {
        if let Some(mut lease) = self.leases.get_mut(&region_id) {
            if lease.owner_id == current_owner || current_owner == 0 {
                lease.owner_id = new_owner;
                lease.readers.clear();
                if let Some(mut region) = self.regions.get_mut(&region_id) {
                    region.owner_id = new_owner;
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    // ── Data access ───────────────────────────────────────────

    pub fn write(&self, region_id: RegionId, offset: u64, data: &[u8], requestor_id: u32) -> anyhow::Result<()> {
        let lease = self.leases.get(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;
        if !lease.can_write(requestor_id) {
            return Err(anyhow::anyhow!("Unauthorized write to region {} by {}", region_id, requestor_id));
        }
        drop(lease);

        let mut region = self.regions.get_mut(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;
        if region.readonly && requestor_id != 0 {
            return Err(anyhow::anyhow!("Region {} is readonly", region_id));
        }

        match &mut region.backing {
            DataBacking::Cpu(ref mut vec) => {
                let end = offset as usize + data.len();
                if end > vec.len() { vec.resize(end, 0); }
                vec[offset as usize..end].copy_from_slice(data);
            }
            DataBacking::Buffer(ref buffer) => {
                let q = self.queue.lock().unwrap();
                q.write_buffer(buffer, offset, data);
            }
            DataBacking::Mapped(ref buffer) => {
                let end = offset as usize + data.len();
                let slice = buffer.slice(offset..end as u64);
                let mut view = slice.get_mapped_range_mut();
                view[..data.len()].copy_from_slice(data);
            }
            DataBacking::Texture { ref texture, width, height, format } => {
                let bpp = bytes_per_pixel(*format);
                let bytes_per_row = bpp * *width;
                let q = self.queue.lock().unwrap();
                q.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d { width: *width, height: *height, depth_or_array_layers: 1 },
                );
            }
        }
        Ok(())
    }

    pub fn read(&self, region_id: RegionId, offset: u64, size: u64, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }

        let lease = self.leases.get(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;
        if !lease.can_read(requestor_id) {
            return Err(anyhow::anyhow!("Unauthorized read of region {} by {}", region_id, requestor_id));
        }
        drop(lease);

        let region = self.regions.get(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;

        match &region.backing {
            DataBacking::Cpu(vec) => {
                let start = offset as usize;
                let end = (offset + size) as usize;
                if end > vec.len() { return Err(anyhow::anyhow!("Read out of bounds")); }
                Ok(vec[start..end].to_vec())
            }
            DataBacking::Buffer(buffer) | DataBacking::Mapped(buffer) => {
                let buffer = buffer.clone();
                drop(region);
                {
                    let q = self.queue.lock().unwrap();
                    q.submit([]);
                    let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                }
                let aligned_size = (size + 3) & !3;
                if buffer.usage().contains(wgpu::BufferUsages::MAP_READ) && offset + aligned_size <= buffer.size() {
                    let slice = buffer.slice(offset..(offset + aligned_size));
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
                        label: Some("Arena-Staging-Read"),
                        size: aligned_size,
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
                    let slice = staging.slice(..aligned_size);
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
            }
            DataBacking::Texture { .. } => {
                Err(anyhow::anyhow!("Direct read from texture regions is not supported"))
            }
        }
    }

    pub fn exists(&self, region_id: RegionId) -> bool {
        self.regions.contains_key(&region_id)
    }

    // ── Lookup helpers (for GpuService / main.rs) ────────────

    pub fn get_buffer(&self, region_id: RegionId) -> Option<Arc<wgpu::Buffer>> {
        self.regions.get(&region_id).and_then(|r| match &r.backing {
            DataBacking::Buffer(b) | DataBacking::Mapped(b) => Some(b.clone()),
            _ => None,
        })
    }

    pub fn get_texture(&self, region_id: RegionId) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        self.regions.get(&region_id).and_then(|r| match &r.backing {
            DataBacking::Texture { texture, width, height, format } => {
                Some((texture.clone(), *width, *height, *format))
            }
            _ => None,
        })
    }

    pub fn get_cpu_data(&self, region_id: RegionId) -> Option<Vec<u8>> {
        self.regions.get(&region_id).and_then(|r| match &r.backing {
            DataBacking::Cpu(v) => Some(v.clone()),
            _ => None,
        })
    }

    // ── Lifecycle ─────────────────────────────────────────────

    pub fn free(&self, region_id: RegionId, requestor_id: u32) -> bool {
        let can_free = self.leases.get(&region_id)
            .map(|l| l.owner_id == requestor_id || requestor_id == 0)
            .unwrap_or(false);
        if can_free {
            self.regions.remove(&region_id);
            self.leases.remove(&region_id);
            true
        } else {
            false
        }
    }

    pub fn compute_hash(&self, region_id: RegionId, requestor_id: u32) -> Option<Vec<u8>> {
        let lease = self.leases.get(&region_id)?;
        if !lease.can_read(requestor_id) { return None; }
        drop(lease);

        let region = self.regions.get(&region_id)?;
        match &region.backing {
            DataBacking::Cpu(v) => Some(blake3::hash(v).as_bytes().to_vec()),
            DataBacking::Buffer(_) | DataBacking::Mapped(_) => {
                let size = region.backing.byte_len();
                drop(region);
                self.read(region_id, 0, size, requestor_id).ok()
                    .map(|data| blake3::hash(&data).as_bytes().to_vec())
            }
            _ => None,
        }
    }
}
