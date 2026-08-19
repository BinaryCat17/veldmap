use std::sync::Arc;
use crate::format::{bytes_per_pixel, proto_to_wgpu};
use crate::registry::{ResourceRegistry, ResourceId, ResourcePayload, GpuObject};

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
    /// `offset + size <= len()` (см. `MemoryManager::read`). Клампить
    /// повторно не нужно — иначе правило хвоста у каждого носителя своё.
    fn read_at(&self, offset: u64, size: u64) -> anyhow::Result<Vec<u8>>;
}

/// Файл на диске: байты остаются на нём, читаются по смещению. Так открываются
/// ресурсы, которые в память не влезают (гигабайтные снимки).
///
/// Только на чтение: файл открывается `File::open`, а пишет файлы модуль fs
/// топиком fs/write, а не владелец ресурса.
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

/// Байты ресурса: всё, у чего работает `read(offset, size)`. Непрозрачные
/// GPU-объекты (текстуры, view, пайплайны) сюда не входят — они в
/// [`GpuObject`].
pub enum DataBacking {
    /// Обычная память хоста.
    Cpu(Vec<u8>),
    /// Носитель, читаемый диапазонами: файл на диске или удалённый ресурс
    /// (см. `RangeSource`). Один вариант на оба потому, что для читателя они
    /// неразличимы — на этом стоит чтение удалённых снимков окнами.
    Range(Arc<dyn RangeSource>),
    /// Буфер GPU. `mapped` — создан с mapped_at_creation, запись идёт прямо
    /// в отображённый диапазон, а не через очередь.
    Buffer { buffer: Arc<wgpu::Buffer>, mapped: bool },
}

impl DataBacking {
    /// Чтение уходит наружу (диск, сеть, ожидание GPU) и может занять поток
    /// надолго. Такие вызовы хост выполняет на blocking-пуле: иначе медленный
    /// носитель съедает воркер рантайма, а не только фибру своего плагина.
    ///
    /// Парного `write_blocks` нет: писать умеют Cpu (memcpy) и GPU (через
    /// очередь wgpu), и ни то, ни другое не блокирует.
    pub fn read_blocks(&self) -> bool {
        matches!(self, Self::Range(_) | Self::Buffer { .. })
    }

    pub fn byte_len(&self) -> u64 {
        match self {
            Self::Cpu(v) => v.len() as u64,
            Self::Range(src) => src.len(),
            Self::Buffer { buffer, .. } => buffer.size(),
        }
    }
}

/// По какой границе wgpu копирует буферы: и смещение, и длину
/// (`COPY_BUFFER_ALIGNMENT`). Невыровненное он отвергает ошибкой валидации, а
/// та уходит в лог и процесс не роняет (`on_uncaptured_error` в setup.rs) —
/// то есть отказ проходит незамеченным, а вызывающий получает нули как
/// удавшееся чтение. Поэтому граница держится здесь, до вызова.
const COPY_ALIGN: u64 = 4;

/// По какой границе wgpu отображает буфер в память (`MAP_ALIGNMENT`). Крупнее
/// копирования вдвое, поэтому окно чтения и строится по той из двух, которая
/// нужна выбранному пути.
const MAP_ALIGN: u64 = 8;

/// Размер, дотянутый до границы.
fn aligned(size: u64) -> u64 {
    size.next_multiple_of(COPY_ALIGN)
}

/// Окно, которым берут с GPU запрошенное `[offset, offset + size)`: начало,
/// длина и сколько байт в начале лишние.
///
/// Расширяется наружу, а не отвергается: смещение и длину называет модуль, то
/// есть любые, а GPU отдаёт только выровненное. Отвергнув, мы объявили бы, что
/// середину буфера прочитать нельзя. Записи это не касается — там расширить
/// окно значит затереть чужие байты, и там отказ единственно верен.
///
/// `limit` — размер самого буфера: он выровнен по [`COPY_ALIGN`] уже при
/// выделении, поэтому обрезка о него границу не портит.
///
/// Запрошенное обязано лежать в буфере (`offset + size <= limit`): окно растёт
/// наружу, а обрезается только о `limit`, и за краем ему взяться неоткуда.
/// Вызывающий это и обеспечивает — размер зажимается длиной носителя раньше.
fn window(offset: u64, size: u64, align: u64, limit: u64) -> (u64, u64, usize) {
    let from = offset - offset % align;
    let to = (offset + size).next_multiple_of(align).min(limit);
    (from, to - from, (offset - from) as usize)
}

/// Выделение памяти, разделяемой хостом с модулями.
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

    // ── Выделение ─────────────────────────────────────────────

    fn alloc(&self, backing: DataBacking, owner_id: u32) -> ResourceId {
        self.registry.register(ResourcePayload::Data(backing), owner_id)
    }

    /// Обнулённая область в куче хоста. 0 — размер не по силам: число приходит
    /// из wasm, и без потолка `Vec` такого размера роняет процесс целиком —
    /// хост вместе со всеми остальными модулями. Потолок общий с линейной
    /// памятью инстанса (см. [`crate::INSTANCE_MEMORY_LIMIT`]): поручить хосту
    /// больше, чем модулю позволено держать самому, он не вправе.
    ///
    /// Проверка здесь, а не у вызывающего, — по той же причине, что и у
    /// [`Self::alloc_texture`]: отказ должен доехать до модуля значением, а не
    /// снять процесс на пути к нему.
    pub fn alloc_cpu(&self, size: u64, owner_id: u32) -> ResourceId {
        if size > crate::INSTANCE_MEMORY_LIMIT {
            log::warn!(target: "memory",
                "Область CPU в {} байт отвергнута: потолок {}", size, crate::INSTANCE_MEMORY_LIMIT);
            return 0;
        }
        self.alloc(DataBacking::Cpu(vec![0u8; size as usize]), owner_id)
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

    /// 0 — размер не по силам устройству. Проверка по той же причине, что и у
    /// [`Self::alloc_texture`]: create_buffer сверх лимита — ошибка валидации
    /// wgpu, а её обработчик по умолчанию роняет процесс.
    pub fn alloc_buffer(&self, size: u64, usage: u32, mapped: bool, owner_id: u32) -> ResourceId {
        let max = self.device.limits().max_buffer_size;
        if size > max {
            log::warn!(target: "memory", "Буфер в {} байт отвергнут: потолок {}", size, max);
            return 0;
        }
        let mut final_usage = wgpu::BufferUsages::from_bits_truncate(usage);
        if !mapped { final_usage |= wgpu::BufferUsages::COPY_DST; }
        if mapped { final_usage |= wgpu::BufferUsages::MAP_WRITE; }
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("memory-buf"), // id ресурса выдаёт реестр, а он ещё впереди
            size: aligned(size),
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
                "Текстура {}x{} отвергнута: потолок {}x{}", width, height, max, max);
            return 0;
        }
        let format = proto_to_wgpu(format_proto);
        // COPY_DST добавляется всем, кроме буфера глубины: копировать в него
        // нечего (см. `upload_image`), а объявленное лишнее право сужает выбор
        // раскладки, которую драйвер вправе дать текстуре.
        let mut final_usage = wgpu::TextureUsages::from_bits_truncate(usage)
            | wgpu::TextureUsages::TEXTURE_BINDING;
        if !crate::format::is_depth(format_proto) {
            final_usage |= wgpu::TextureUsages::COPY_DST;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("memory-tex"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: final_usage,
            view_formats: &[],
        });
        self.registry.register(
            ResourcePayload::Gpu(GpuObject::Texture {
                texture: Arc::new(texture), width, height, format: format_proto,
            }),
            owner_id,
        )
    }

    // ── Доступ к данным ───────────────────────────────────────
    // Проверки доступа выполняет вызывающий через ResourceRegistry
    // (см. abi.rs); здесь — только байтовые операции с носителем.

    /// Записывает байты по смещению. Только для байтовых носителей: у
    /// текстуры смещение не определено, и заливается она [`Self::upload_image`].
    pub fn write(&self, region_id: ResourceId, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        // Конец записи — то, что решает судьбу всех трёх носителей, и считать
        // его надо один раз: смещение приходит из wasm, и `offset + len`
        // переполняет `usize` раньше, чем что-либо успевает его проверить.
        let end = (offset as u128) + (data.len() as u128);
        self.registry.payload_mut(region_id, |payload| match payload {
            ResourcePayload::Data(backing) => {
                match backing {
                    DataBacking::Cpu(vec) => {
                        // Область растёт под запись — иначе модулю пришлось бы
                        // знать её конец заранее, — но растёт до того же
                        // потолка, что и `alloc_cpu`: без него одна запись по
                        // смещению в терабайт роняет хост со всеми остальными
                        // модулями, а поручить ему больше, чем модулю
                        // позволено держать самому, никто не вправе.
                        if end > u128::from(crate::INSTANCE_MEMORY_LIMIT) {
                            return Err(anyhow::anyhow!(
                                "запись до {} байт при потолке {}",
                                end,
                                crate::INSTANCE_MEMORY_LIMIT
                            ));
                        }
                        let end = end as usize;
                        if end > vec.len() { vec.resize(end, 0); }
                        vec[offset as usize..end].copy_from_slice(data);
                    }
                    // Буфер GPU не растёт: размер ему назначен при выделении.
                    // Выход за него wgpu разбирает сам — ошибкой валидации у
                    // очереди и паникой у отображённого среза, — а и то, и
                    // другое снимает процесс. Довод пришёл из wasm, значит
                    // проверяем мы.
                    DataBacking::Buffer { buffer, .. } if end > u128::from(buffer.size()) => {
                        return Err(anyhow::anyhow!(
                            "запись до {} байт в буфер размером {}",
                            end,
                            buffer.size()
                        ));
                    }
                    DataBacking::Buffer { buffer, mapped: false } => {
                        // Очередь принимает только выровненное (см.
                        // [`COPY_ALIGN`]). Расширить окно здесь нельзя —
                        // затёрлись бы чужие байты, — поэтому отказ.
                        if offset % COPY_ALIGN != 0 || data.len() as u64 % COPY_ALIGN != 0 {
                            return Err(anyhow::anyhow!(
                                "запись в буфер не выровнена по {} байтам: смещение {}, длина {}",
                                COPY_ALIGN,
                                offset,
                                data.len()
                            ));
                        }
                        let q = self.queue.lock().unwrap();
                        q.write_buffer(buffer, offset, data);
                    }
                    DataBacking::Buffer { buffer, mapped: true } => {
                        let end = offset + data.len() as u64;
                        let slice = buffer.slice(offset..end);
                        // Отображённая на запись память — write-combining, и
                        // читать её нельзя, поэтому доступ только через
                        // `slice(..)`: обычного `&mut [u8]` у неё больше нет.
                        let mut view = slice.get_mapped_range_mut()?;
                        view.slice(..data.len()).copy_from_slice(data);
                    }
                    // Для файла запись идёт топиком fs/write, для удалённого
                    // ресурса это отдельный протокол (PUT, права на той
                    // стороне) — не «ещё один вариант write».
                    DataBacking::Range(_) => {
                        return Err(anyhow::anyhow!("диапазонный носитель доступен только на чтение"));
                    }
                }
                Ok(())
            }
            ResourcePayload::Gpu(GpuObject::Texture { .. }) => {
                Err(anyhow::anyhow!("ресурс {} — текстура: заливать её надо upload_image", region_id))
            }
            // Прочие GPU-объекты байт не несут вовсе — и это не «не найден»:
            // ресурс есть, читать в нём нечего.
            ResourcePayload::Gpu(_) => {
                Err(anyhow::anyhow!("ресурс {} — GPU-объект, байт за ним нет", region_id))
            }
        }).ok_or_else(|| anyhow::anyhow!("Ресурс {} не найден", region_id))?
    }

    /// Заливает изображение в текстуру целиком.
    ///
    /// Отдельная операция, а не `write` со смещением 0: смещения у текстуры
    /// нет, частичной заливки нет, и данные обязаны покрывать её всю — то
    /// есть от записи по смещению здесь не остаётся ни одного параметра.
    pub fn upload_image(&self, region_id: ResourceId, data: &[u8]) -> anyhow::Result<()> {
        self.registry.payload(region_id, |payload| match payload {
            ResourcePayload::Gpu(GpuObject::Texture { texture, width, height, format }) => {
                if crate::format::is_depth(*format) {
                    return Err(anyhow::anyhow!(
                        "ресурс {} — буфер глубины: в него пишет только растеризатор", region_id));
                }
                let expected = (bytes_per_pixel(*format) * *width) as u64 * (*height as u64);
                if (data.len() as u64) < expected {
                    return Err(anyhow::anyhow!(
                        "в изображении {} байт, а текстуре {}x{} нужно {}",
                        data.len(), width, height, expected));
                }
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
                        bytes_per_row: Some(bytes_per_pixel(*format) * *width),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d { width: *width, height: *height, depth_or_array_layers: 1 },
                );
                Ok(())
            }
            _ => Err(anyhow::anyhow!("ресурс {} — не текстура", region_id)),
        }).ok_or_else(|| anyhow::anyhow!("Ресурс {} не найден", region_id))?
    }

    /// Байты ресурса со смещения.
    ///
    /// Чтение за концом — не ошибка, а короткий (возможно пустой) ответ, как
    /// у файла: читатель идёт окнами, и последнее окно почти всегда неполное
    /// (`ResourceReader` в SDK). Правило одно на все носители и проверяется
    /// здесь, а не в каждом из них.
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
                let len = backing.byte_len();
                if offset >= len { return Ok((0, Source::Bytes(Vec::new()))); }
                let size = size.min(len - offset);
                Ok((size, match backing {
                    DataBacking::Cpu(vec) => {
                        Source::Bytes(vec[offset as usize..(offset + size) as usize].to_vec())
                    }
                    DataBacking::Range(source) => Source::Range(source.clone()),
                    DataBacking::Buffer { buffer, .. } => Source::Buffer(buffer.clone()),
                }))
            }
            // Смещение у текстуры не определено, и копия GPU→CPU остановила бы
            // конвейер: превью снимают, рисуя текстуру, а не вычитывая её.
            ResourcePayload::Gpu(GpuObject::Texture { .. }) => {
                Err(anyhow::anyhow!("прямое чтение текстуры не поддерживается"))
            }
            // Как и при чтении: ресурс есть, байт за ним нет.
            ResourcePayload::Gpu(_) => {
                Err(anyhow::anyhow!("ресурс {} — GPU-объект, байт за ним нет", region_id))
            }
        }).ok_or_else(|| anyhow::anyhow!("Ресурс {} не найден", region_id))??;

        match source {
            Source::Bytes(data) => Ok(data),
            Source::Range(source) => source.read_at(offset, size),
            // Чтение из GPU-буфера — два пути с одним смыслом: дождаться
            // очереди, отобразить диапазон в память и скопировать. MAP_READ
            // читается на месте; всякий другой буфер сначала копируется в
            // staging (маппить можно только созданное под маппинг). Пустой
            // submit перед ожиданием проталкивает недосабмиченное — иначе
            // poll ждать нечего, а в буфере лежит прошлое.
            Source::Buffer(buffer) => {
                {
                    let q = self.queue.lock().unwrap();
                    q.submit([]);
                    let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                }
                if buffer.usage().contains(wgpu::BufferUsages::MAP_READ) {
                    let (from, len, skip) = window(offset, size, MAP_ALIGN, buffer.size());
                    let slice = buffer.slice(from..(from + len));
                    let (tx, rx) = std::sync::mpsc::channel();
                    slice.map_async(wgpu::MapMode::Read, move |res| { let _ = tx.send(res); });
                    {
                        let _q = self.queue.lock().unwrap();
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    rx.recv()??;
                    let data = slice.get_mapped_range()?[skip..skip + size as usize].to_vec();
                    buffer.unmap();
                    Ok(data)
                } else {
                    // Копировать можно только то, что объявлено источником
                    // копии. Проверяем сами, потому что отказ wgpu уходит в лог
                    // (см. `on_uncaptured_error` в setup.rs), staging остаётся
                    // нулевым, и модуль получает нули как удавшееся чтение —
                    // худший из возможных ответов.
                    if !buffer.usage().contains(wgpu::BufferUsages::COPY_SRC) {
                        return Err(anyhow::anyhow!(
                            "буфер {} не читается: выделен без COPY_SRC и без MAP_READ",
                            region_id
                        ));
                    }
                    let (from, len, skip) = window(offset, size, COPY_ALIGN, buffer.size());
                    let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Memory-Staging-Read"),
                        size: len,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    encoder.copy_buffer_to_buffer(&buffer, from, &staging, 0, len);
                    {
                        let q = self.queue.lock().unwrap();
                        q.submit(Some(encoder.finish()));
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    let slice = staging.slice(..len);
                    let (tx, rx) = std::sync::mpsc::channel();
                    slice.map_async(wgpu::MapMode::Read, move |res| { let _ = tx.send(res); });
                    {
                        let _q = self.queue.lock().unwrap();
                        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                    }
                    rx.recv()??;
                    let data = slice.get_mapped_range()?[skip..skip + size as usize].to_vec();
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

    // ── Поиск носителя ────────────

    pub fn get_buffer(&self, region_id: ResourceId) -> Option<Arc<wgpu::Buffer>> {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Data(DataBacking::Buffer { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        }).flatten()
    }

    pub fn get_texture(&self, region_id: ResourceId) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        self.registry.payload(region_id, |p| match p {
            ResourcePayload::Gpu(GpuObject::Texture { texture, width, height, format }) => {
                Some((texture.clone(), *width, *height, *format))
            }
            _ => None,
        }).flatten()
    }

    // Освобождения отдельного метода нет: запись о ресурсе одна, поэтому и
    // освобождение одно — `ResourceRegistry::unregister`.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Размер буфера дотягивается до границы копирования, а не отвергается:
    /// просить пять байт законно, копировать пять — нет.
    #[test]
    fn size_reaches_the_copy_boundary() {
        assert_eq!(aligned(0), 0);
        assert_eq!(aligned(1), 4);
        assert_eq!(aligned(4), 4);
        assert_eq!(aligned(5), 8);
    }

    /// Окно накрывает запрошенное целиком и попадает на границу обоими
    /// концами: невыровненный конец — та же ошибка валидации, что и начало.
    #[test]
    fn the_window_covers_what_was_asked_and_lands_on_the_boundary() {
        let limit = 1024;
        for align in [COPY_ALIGN, MAP_ALIGN] {
            for offset in 0..24_u64 {
                for size in 1..24_u64 {
                    let (from, len, skip) = window(offset, size, align, limit);
                    assert_eq!(from % align, 0, "начало {} при выравнивании {}", from, align);
                    assert_eq!(from + skip as u64, offset, "смещение внутри окна");
                    assert!(skip as u64 + size <= len, "окно короче запрошенного: {} < {}", len, skip as u64 + size);
                    assert!(from + len <= limit, "окно вышло за буфер");
                }
            }
        }
    }

    /// У края буфера окно обрезается его размером, а не вылезает наружу.
    /// Граница при этом остаётся: сам буфер выровнен уже при выделении.
    #[test]
    fn the_window_stops_at_the_end_of_the_buffer() {
        let limit = aligned(30);
        let (from, len, skip) = window(29, 3, COPY_ALIGN, limit);
        assert_eq!(from + len, limit, "окно обязано доходить до конца буфера");
        assert_eq!(from % COPY_ALIGN, 0);
        assert!(skip as u64 + 3 <= len);
    }

    /// Нулевой размер окна не растит: читать нечего, и просить у GPU нечего.
    #[test]
    fn an_empty_window_stays_empty() {
        assert_eq!(window(8, 0, COPY_ALIGN, 1024), (8, 0, 0));
    }
}
