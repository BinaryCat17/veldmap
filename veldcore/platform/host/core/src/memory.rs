use std::sync::Arc;
use dashmap::DashMap;
use crate::graphics::proto::TextureFormat;
use crate::registry::{ResourceRegistry, ResourceId, ResourceBackend};

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

/// Чем подкреплены байты ресурса.
///
/// Ресурс один — id, владение (lease) и освобождение у всех вариантов общие;
/// различается только носитель и, как следствие, набор доступных операций:
/// `read`/`write` по смещению работают для Cpu/File/Buffer, а Texture — это
/// не байтовый диапазон (чтение потребовало бы копии GPU→CPU со стопом
/// конвейера), поэтому у неё только запись целого изображения.
pub enum DataBacking {
    /// Обычная память хоста.
    Cpu(Vec<u8>),
    /// Файл на диске: байты читаются и пишутся по смещению, целиком в память
    /// не поднимаются. Так открываются ресурсы, которые в память не влезают
    /// (гигабайтные снимки), — потребитель тянет из них нужные фрагменты.
    File { file: std::sync::Mutex<std::fs::File>, len: u64 },
    /// Буфер GPU. `mapped` — создан с mapped_at_creation, запись идёт прямо
    /// в отображённый диапазон, а не через очередь.
    Buffer { buffer: Arc<wgpu::Buffer>, mapped: bool },
    Texture {
        texture: Arc<wgpu::Texture>,
        width: u32,
        height: u32,
        format: i32,
    },
}

impl DataBacking {
    pub fn is_buffer(&self) -> bool {
        matches!(self, Self::Buffer { .. })
    }

    pub fn as_buffer(&self) -> Option<&wgpu::Buffer> {
        match self {
            Self::Buffer { buffer, .. } => Some(buffer),
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
            Self::File { len, .. } => *len,
            Self::Buffer { buffer, .. } => buffer.size(),
            Self::Texture { width, height, format, .. } => {
                let bpp = bytes_per_pixel(*format);
                (*width as u64) * (*height as u64) * (bpp as u64)
            }
        }
    }
}

/// A data-bearing region
pub struct Region {
    pub backing: DataBacking,
}

/// Host-managed shared memory manager: raw allocator
pub struct MemoryManager {
    regions: DashMap<ResourceId, Region>,
    registry: Arc<ResourceRegistry>,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
}

impl MemoryManager {
    pub fn new(registry: Arc<ResourceRegistry>, device: Arc<wgpu::Device>, queue: Arc<std::sync::Mutex<wgpu::Queue>>) -> Self {
        Self {
            regions: DashMap::new(),
            registry,
            device,
            queue,
        }
    }

    // ── Allocation ────────────────────────────────────────────

    fn alloc(&self, backing: DataBacking, owner_id: u32) -> ResourceId {
        let id = self.registry.register(ResourceBackend::Memory, owner_id, None);
        self.regions.insert(id, Region { backing });
        id
    }

    pub fn alloc_cpu(&self, data: Vec<u8>, owner_id: u32) -> ResourceId {
        self.alloc(DataBacking::Cpu(data), owner_id)
    }

    /// Ресурс поверх файла: содержимое остаётся на диске, чтение и запись
    /// идут по смещению (см. `DataBacking::File`). Размер фиксируется на
    /// момент открытия — он же уезжает потребителю в ResourceHandle.size.
    pub fn alloc_file(&self, path: &std::path::Path, owner_id: u32) -> std::io::Result<(ResourceId, u64)> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let id = self.alloc(DataBacking::File { file: std::sync::Mutex::new(file), len }, owner_id);
        Ok((id, len))
    }

    pub fn alloc_buffer(&self, size: u64, usage: u32, mapped: bool, owner_id: u32) -> ResourceId {
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !mapped { final_usage |= wgpu::BufferUsages::COPY_DST; }
        if mapped { final_usage |= wgpu::BufferUsages::MAP_WRITE; }
        let aligned = (size + 3) & !3;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("memory-buf"), // We don't have the ID yet before registering
            size: aligned,
            usage: final_usage,
            mapped_at_creation: mapped,
        });
        self.alloc(DataBacking::Buffer { buffer: Arc::new(buffer), mapped }, owner_id)
    }

    /// 0 — размеры не по силам устройству. Проверка здесь, а не у вызывающего:
    /// create_texture на превышении лимита — ошибка валидации wgpu, а её
    /// обработчик по умолчанию роняет процесс, тогда как модулю достаточно
    /// узнать, что текстуру выделить не удалось.
    pub fn alloc_texture(&self, width: u32, height: u32, format_proto: i32, usage: u32, owner_id: u32) -> ResourceId {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            log::warn!(target: "veldmap::host::memory",
                "Texture {}x{} rejected: limit is {}x{}", width, height, max, max);
            return 0;
        }
        let format = proto_to_wgpu_format(format_proto);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("memory-tex"),
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
            owner_id,
        )
    }

    // ── Data access ───────────────────────────────────────────
    // Note: Security checks are performed by the caller using ResourceRegistry.
    // This is just a raw byte store.

    pub fn write(&self, region_id: ResourceId, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        let mut region = self.regions.get_mut(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;
        
        match &mut region.backing {
            DataBacking::Cpu(ref mut vec) => {
                let end = offset as usize + data.len();
                if end > vec.len() { vec.resize(end, 0); }
                vec[offset as usize..end].copy_from_slice(data);
            }
            DataBacking::File { file, len } => {
                use std::io::{Seek, SeekFrom, Write};
                let mut f = file.lock().unwrap();
                f.seek(SeekFrom::Start(offset))?;
                f.write_all(data)?;
                *len = (*len).max(offset + data.len() as u64);
            }
            DataBacking::Buffer { buffer, mapped: false } => {
                let q = self.queue.lock().unwrap();
                q.write_buffer(buffer, offset, data);
            }
            DataBacking::Buffer { buffer, mapped: true } => {
                let end = offset + data.len() as u64;
                let slice = buffer.slice(offset..end);
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

    pub fn read(&self, region_id: ResourceId, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }

        let region = self.regions.get(&region_id)
            .ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?;

        match &region.backing {
            DataBacking::Cpu(vec) => {
                let start = offset as usize;
                let end = (offset + size) as usize;
                if end > vec.len() { return Err(anyhow::anyhow!("Read out of bounds")); }
                Ok(vec[start..end].to_vec())
            }
            // Хвост короче запрошенного — не ошибка, а конец файла: читатель
            // (SDK Resource) идёт окнами и последнее окно почти всегда неполное.
            DataBacking::File { file, len } => {
                if offset >= *len { return Ok(Vec::new()); }
                let size = size.min(*len - offset) as usize;
                use std::io::{Read, Seek, SeekFrom};
                let mut f = file.lock().unwrap();
                f.seek(SeekFrom::Start(offset))?;
                let mut buf = vec![0u8; size];
                f.read_exact(&mut buf)?;
                Ok(buf)
            }
            DataBacking::Buffer { buffer, .. } => {
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
                        label: Some("Memory-Staging-Read"),
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

    pub fn exists(&self, region_id: ResourceId) -> bool {
        self.regions.contains_key(&region_id)
    }

    pub fn get_size(&self, region_id: ResourceId) -> u64 {
        if let Some(r) = self.regions.get(&region_id) {
            r.backing.byte_len()
        } else {
            0
        }
    }

    // ── Lookup helpers ────────────

    pub fn get_buffer(&self, region_id: ResourceId) -> Option<Arc<wgpu::Buffer>> {
        self.regions.get(&region_id).and_then(|r| match &r.backing {
            DataBacking::Buffer { buffer, .. } => Some(buffer.clone()),
            _ => None,
        })
    }

    pub fn get_texture(&self, region_id: ResourceId) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        self.regions.get(&region_id).and_then(|r| match &r.backing {
            DataBacking::Texture { texture, width, height, format } => {
                Some((texture.clone(), *width, *height, *format))
            }
            _ => None,
        })
    }

    // ── Lifecycle ─────────────────────────────────────────────

    pub fn free(&self, region_id: ResourceId) -> bool {
        // Validation happens in the caller (System or host module)
        if self.regions.remove(&region_id).is_some() {
            self.registry.unregister(region_id);
            true
        } else {
            false
        }
    }

}
