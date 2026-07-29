use std::sync::Arc;
use crate::graphics::proto::TextureFormat;
use crate::registry::{ResourceRegistry, ResourceId, ResourcePayload};

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

/// Носитель, умеющий отдавать произвольный диапазон байт.
///
/// Реализация живёт там, где известен протокол: файл на диске — здесь же
/// (`FileSource`), HTTP с Range-запросами — в модуле network, чтобы ядро не
/// тянуло в себя http-клиент. Для читателя разницы нет, и это не совпадение,
/// а условие: код, идущий по ресурсу окнами, работает с диском и с сетью без
/// единой правки. Чтение блокирующее — хост вызывает его на blocking-пуле,
/// см. `DataBacking::read_blocks`.
pub trait RangeSource: Send + Sync {
    fn len(&self) -> u64;

    /// Диапазон уже проверен вызывающим: `offset < len()` и
    /// `offset + size <= len()` (см. `MemoryManager::read`). Реализациям
    /// клампить повторно не нужно — раньше это делал каждый носитель сам,
    /// и делал по-своему.
    fn read_at(&self, offset: u64, size: u64) -> anyhow::Result<Vec<u8>>;
}

/// Файл на диске: байты остаются на нём, читаются по смещению. Так открываются
/// ресурсы, которые в память не влезают (гигабайтные снимки).
///
/// Открыт только на чтение — записи через ресурс у файла нет и не было:
/// ветка записи существовала, но `alloc_file` открывает файл `File::open`, и
/// любая запись возвращала бы ошибку дескриптора. Файлы пишет модуль fs
/// (топик fs/write), а не владелец ресурса.
struct FileSource {
    file: std::sync::Mutex<std::fs::File>,
    len: u64,
}

impl RangeSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; size as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
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
    /// Носитель, читаемый диапазонами: файл на диске или удалённый ресурс
    /// (см. `RangeSource`). Один вариант на оба именно потому, что для
    /// читателя они одинаковы; двумя вариантами они были ровно до тех пор,
    /// пока клампинг хвоста и признак «чтение блокирует» не расползлись по
    /// двум копиям.
    Range(Arc<dyn RangeSource>),
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

    /// Чтение уходит наружу (диск, сеть, ожидание GPU) и может занять поток
    /// надолго. Такие вызовы хост выполняет на blocking-пуле: иначе медленный
    /// носитель съедает воркер рантайма, а не только фибру своего плагина.
    ///
    /// Парного `write_blocks` нет: писать умеют только Cpu (memcpy) и GPU
    /// (через очередь wgpu), а они не блокируют. Диапазонный носитель
    /// доступен лишь на чтение.
    pub fn read_blocks(&self) -> bool {
        matches!(self, Self::Range(_) | Self::Buffer { .. })
    }

    pub fn byte_len(&self) -> u64 {
        match self {
            Self::Cpu(v) => v.len() as u64,
            Self::Range(src) => src.len(),
            Self::Buffer { buffer, .. } => buffer.size(),
            Self::Texture { width, height, format, .. } => {
                let bpp = bytes_per_pixel(*format);
                (*width as u64) * (*height as u64) * (bpp as u64)
            }
        }
    }
}

/// Host-managed shared memory manager: raw allocator
///
/// Сам менеджер ничего не хранит: записи (носитель + lease) живут в
/// реестре, здесь только создание носителей и байтовые операции над ними.
pub struct MemoryManager {
    registry: Arc<ResourceRegistry>,
    device: Arc<wgpu::Device>,
    queue: Arc<std::sync::Mutex<wgpu::Queue>>,
}

impl MemoryManager {
    pub fn new(registry: Arc<ResourceRegistry>, device: Arc<wgpu::Device>, queue: Arc<std::sync::Mutex<wgpu::Queue>>) -> Self {
        Self {
            registry,
            device,
            queue,
        }
    }

    // ── Allocation ────────────────────────────────────────────

    fn alloc(&self, backing: DataBacking, owner_id: u32) -> ResourceId {
        self.registry.register(ResourcePayload::Data(backing), owner_id)
    }

    pub fn alloc_cpu(&self, data: Vec<u8>, owner_id: u32) -> ResourceId {
        self.alloc(DataBacking::Cpu(data), owner_id)
    }

    /// Ресурс поверх файла: содержимое остаётся на диске, читатель тянет
    /// диапазоны. Размер фиксируется на момент открытия — он же уезжает
    /// потребителю в ResourceHandle.size.
    pub fn alloc_file(&self, path: &std::path::Path, owner_id: u32) -> std::io::Result<(ResourceId, u64)> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let source = FileSource { file: std::sync::Mutex::new(file), len };
        Ok((self.alloc_range(Arc::new(source), owner_id), len))
    }

    /// Ресурс поверх произвольного диапазонного носителя (см. `RangeSource`):
    /// содержимое остаётся на той стороне, читатель тянет диапазоны.
    pub fn alloc_range(&self, source: Arc<dyn RangeSource>, owner_id: u32) -> ResourceId {
        self.alloc(DataBacking::Range(source), owner_id)
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
            log::warn!(target: "memory",
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
    // Проверки доступа выполняет вызывающий через ResourceRegistry
    // (см. abi.rs); здесь — только байтовые операции с носителем.

    pub fn write(&self, region_id: ResourceId, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        self.registry.payload_mut(region_id, |payload| match payload {
            ResourcePayload::Data(backing) => {
                match backing {
                    DataBacking::Cpu(vec) => {
                        let end = offset as usize + data.len();
                        if end > vec.len() { vec.resize(end, 0); }
                        vec[offset as usize..end].copy_from_slice(data);
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
                    // Диапазонный носитель только читается. Для файла запись идёт
                    // топиком fs/write, для удалённого ресурса это вообще отдельный
                    // протокол (PUT, докачка, права на той стороне) — не «ещё один
                    // вариант write».
                    DataBacking::Range(_) => {
                        return Err(anyhow::anyhow!("Range-backed resources are read-only"));
                    }
                    DataBacking::Texture { texture, width, height, format } => {
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
            // GPU-объект — не байтовый ресурс; для вызывающего это «нет такого региона».
            ResourcePayload::Gpu(_) => Err(anyhow::anyhow!("Region {} not found", region_id)),
        }).ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))?
    }

    /// Байты ресурса со смещения.
    ///
    /// Чтение за концом — не ошибка, а короткий (возможно пустой) ответ, как
    /// у файла: читатель идёт окнами, и последнее окно почти всегда неполное
    /// (`ResourceReader` в SDK). Правило одно на все носители и проверяется
    /// здесь: раньше каждый решал сам, и один и тот же ABI-вызов имел три
    /// разных контракта — Cpu отвечал ошибкой «out of bounds», файл и сеть
    /// возвращали короткий буфер. Держалось это лишь на том, что
    /// единственный читатель клампил запрос у себя.
    pub fn read(&self, region_id: ResourceId, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        if size == 0 { return Ok(Vec::new()); }

        // Что читаем, вынесено из-под guard'а реестра: Cpu копируется сразу
        // (это memcpy), Range/Buffer отдают Arc, а блокирующее чтение идёт
        // уже без guard'а — под ним оно заперло бы шард DashMap для всех
        // остальных обращений к реестру (см. комментарий к ResourceRegistry).
        enum Source {
            Bytes(Vec<u8>),
            Range(Arc<dyn RangeSource>),
            Buffer(Arc<wgpu::Buffer>),
        }

        let (size, source) = self.registry.payload(region_id, |payload| match payload {
            ResourcePayload::Data(backing) => {
                // Текстура — не байтовый диапазон, смещение для неё не определено;
                // отвечаем отказом до всякого клампинга.
                if matches!(backing, DataBacking::Texture { .. }) {
                    return Err(anyhow::anyhow!("Direct read from texture regions is not supported"));
                }
                let len = backing.byte_len();
                if offset >= len { return Ok((0, Source::Bytes(Vec::new()))); }
                let size = size.min(len - offset);
                Ok((size, match backing {
                    DataBacking::Cpu(vec) => {
                        Source::Bytes(vec[offset as usize..(offset + size) as usize].to_vec())
                    }
                    DataBacking::Range(source) => Source::Range(source.clone()),
                    DataBacking::Buffer { buffer, .. } => Source::Buffer(buffer.clone()),
                    // Отсеяна выше, до проверки диапазона.
                    DataBacking::Texture { .. } => unreachable!(),
                }))
            }
            // GPU-объект — не байтовый ресурс; для вызывающего это «нет такого региона».
            ResourcePayload::Gpu(_) => Err(anyhow::anyhow!("Region {} not found", region_id)),
        }).ok_or_else(|| anyhow::anyhow!("Region {} not found", region_id))??;

        match source {
            Source::Bytes(data) => Ok(data),
            Source::Range(source) => source.read_at(offset, size),
            Source::Buffer(buffer) => {
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
        }
    }

    /// Нужно ли выполнять операцию на blocking-пуле (см. `DataBacking::read_blocks`).
    pub fn read_blocks(&self, region_id: ResourceId) -> bool {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Data(backing) => backing.read_blocks(),
            ResourcePayload::Gpu(_) => false,
        }).unwrap_or(false)
    }

    pub fn get_size(&self, region_id: ResourceId) -> u64 {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Data(backing) => backing.byte_len(),
            ResourcePayload::Gpu(_) => 0,
        }).unwrap_or(0)
    }

    // ── Lookup helpers ────────────

    pub fn get_buffer(&self, region_id: ResourceId) -> Option<Arc<wgpu::Buffer>> {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Data(DataBacking::Buffer { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        }).flatten()
    }

    pub fn get_texture(&self, region_id: ResourceId) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Data(DataBacking::Texture { texture, width, height, format }) => {
                Some((texture.clone(), *width, *height, *format))
            }
            _ => None,
        }).flatten()
    }

    // Освобождения отдельного метода нет: запись одна, поэтому освобождение
    // одно — `ResourceRegistry::unregister` (см. `veld_memory_free` в abi.rs).
}
