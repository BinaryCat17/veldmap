//! Типизированная обёртка над графическим сервисом хоста.
//!
//! Буферы и текстуры выделяются через memory ABI (`abi::resource_alloc_*`)
//! и адресуются region id. Здесь создаются непрозрачные GPU-объекты
//! (шейдеры, пайплайны, view, сэмплеры, bind group'ы) — каждый со своим
//! типом идентификатора, чтобы их нельзя было перепутать, — и записываются
//! render-команды через [`RenderRecorder`].

mod buffer;
mod recorder;
pub use buffer::GrowingBuffer;
pub use recorder::RenderRecorder;

/// Контракт `veldmap.graphics` живёт в общем дереве протокола, здесь — только
/// псевдоним: обёртки ниже адресуют свои типы как `proto::*`.
pub use crate::proto::graphics as proto;

pub use proto::{
    TextureFormat, VertexFormat, IndexFormat, FilterMode, StepMode,
    PrimitiveTopology, BlendFactor, BlendOperation, FrontFace, CullMode,
    BufferBindingType, SamplerBindingType, TextureSampleType, TextureViewDimension,
    CompareFunction, VertexAttribute, VertexBufferLayout, BlendComponent, BlendState,
    BindGroupLayoutEntry, BindGroupEntry, CreateRenderPipeline, DepthStencilState,
};

use prost::Message;
use proto::{resource_request, ResourceRequest};

// ── Типизированные идентификаторы GPU-объектов ─────────────────

/// Сырой id GPU-объекта. Drop освобождает объект на хосте: тот же путь,
/// что у memory-регионов (veld_resource_free умеет и то, и другое).
/// Bind group/пайплайн на хосте держат wgpu-ссылки на свои ресурсы,
/// поэтому раннее освобождение view/сэмплера/шейдера после создания
/// bind group/пайплайна безопасно.
#[derive(Debug, PartialEq, Eq, Hash)]
struct RawGpuId(u64);

impl Drop for RawGpuId {
    fn drop(&mut self) {
        crate::abi::resource_free(self.0);
    }
}

macro_rules! gpu_id {
    ($(#[$doc:meta] $name:ident),+ $(,)?) => {$(
        #[$doc]
        ///
        /// Владение разделяемое (Arc): объект на хосте освобождается,
        /// когда умирает последний владелец id.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(std::sync::Arc<RawGpuId>);

        impl $name {
            fn new(id: u64) -> Self { Self(std::sync::Arc::new(RawGpuId(id))) }
            /// Сырой id объекта в реестре хоста.
            pub fn id(&self) -> u64 { self.0.0 }
        }
    )+};
}

gpu_id! {
    /// Скомпилированный шейдерный модуль (WGSL)
    ShaderId,
    /// Render pipeline
    PipelineId,
    /// Сэмплер
    SamplerId,
    /// View текстуры (render target или binding)
    TextureViewId,
    /// Layout bind group'ы
    BindGroupLayoutId,
    /// Bind group
    BindGroupId,
}

// ── Видимость биндингов (битовая маска wgpu::ShaderStages) ─────

pub const VISIBILITY_VERTEX: u32 = 1;
pub const VISIBILITY_FRAGMENT: u32 = 2;
pub const VISIBILITY_COMPUTE: u32 = 4;

/// Битовые флаги wgpu::BufferUsages для `abi::resource_alloc_buffer`.
pub mod buffer_usage {
    pub const MAP_READ: u32 = 1 << 0;
    pub const MAP_WRITE: u32 = 1 << 1;
    pub const COPY_SRC: u32 = 1 << 2;
    pub const COPY_DST: u32 = 1 << 3;
    pub const INDEX: u32 = 1 << 4;
    pub const VERTEX: u32 = 1 << 5;
    pub const UNIFORM: u32 = 1 << 6;
    pub const STORAGE: u32 = 1 << 7;
}

/// Битовые флаги wgpu::TextureUsages для `abi::resource_alloc_texture`.
pub mod texture_usage {
    pub const COPY_SRC: u32 = 1 << 0;
    pub const COPY_DST: u32 = 1 << 1;
    pub const TEXTURE_BINDING: u32 = 1 << 2;
    pub const STORAGE_BINDING: u32 = 1 << 3;
    pub const RENDER_ATTACHMENT: u32 = 1 << 4;
}

// ── Байты для GPU ──────────────────────────────────────────────

/// Срез `Copy`-значений байтами. Раскладку задаёт `#[repr(C)]` у типов,
/// уезжающих на GPU. Единственное место этого unsafe: переписанный на
/// call-сайте, он множится вместе с местами записи.
pub fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data))
    }
}

/// Буфер, залитый срезом целиком: выделение, запись и владелец одной
/// операцией. Для геометрии, собираемой один раз; растущей каждый кадр —
/// [`GrowingBuffer`].
pub fn upload<T: Copy>(what: &str, data: &[T], usage: u32) -> anyhow::Result<crate::OwnedResource> {
    let bytes = bytes_of(data);
    let size = bytes.len() as u64;
    let id = crate::abi::resource_alloc_buffer(size, usage, false)
        .ok_or_else(|| anyhow::anyhow!("не выделился буфер: {} ({} байт)", what, size))?;
    let region = crate::OwnedResource::new(crate::ResourceHandle { id, size });
    crate::abi::resource_write(region.id(), 0, bytes)?;
    Ok(region)
}

/// Готовая картинка → текстура во владении названного сервиса: выделение,
/// заливка и передача владения одной операцией. Пара к [`upload`], только для
/// изображения и с отдачей владения.
///
/// Здесь потому, что два правила обряда молчаливы и оба стоят ресурса:
/// `COPY_DST` — без него заливка откажет; владелец до заливки — иначе
/// сорвавшаяся заливка оставит голый id висеть на хосте до конца процесса.
///
/// А вот **формат — довод, а не умолчание**: чем ляжет картинка, решает тот,
/// кто её отдаёт, и SDK за него не выбирает. Сэмплится готовое содержимое как
/// sRGB, а место под чужой рендер — как UNORM, и правило это не платформы, а
/// того протокола, по которому картинка едет.
pub fn upload_texture(
    what: &str,
    width: u32,
    height: u32,
    format: TextureFormat,
    pixels: &[u8],
    owner: &str,
) -> Result<crate::ResourceHandle, String> {
    let id = crate::abi::resource_alloc_texture(
        width,
        height,
        format as i32,
        texture_usage::TEXTURE_BINDING | texture_usage::COPY_DST,
    )
    .ok_or_else(|| format!("не выделилась текстура {} {}×{}", what, width, height))?;
    let texture = crate::OwnedResource::from_raw_id(id);
    crate::abi::resource_upload_image(texture.id(), pixels)
        .map_err(|e| format!("{} не залит в текстуру: {}", what, e))?;
    crate::resource::hand_off(
        crate::ResourceHandle { id: texture.into_handle().id, size: pixels.len() as u64 },
        owner,
    )
}

// ── Создание ресурсов ──────────────────────────────────────────

fn create(command: resource_request::Command) -> anyhow::Result<u64> {
    let req = ResourceRequest { command: Some(command) };
    let res_bytes = crate::abi::resource_create(req.encode_to_vec())?;
    let id_bytes: [u8; 8] = res_bytes.as_slice().try_into()
        .map_err(|_| anyhow::anyhow!("graphics create_resource: malformed id ({} bytes)", res_bytes.len()))?;
    Ok(u64::from_le_bytes(id_bytes))
}

pub fn create_shader(source: &str, label: &str) -> anyhow::Result<ShaderId> {
    create(resource_request::Command::CreateShader(proto::CreateShaderModule {
        source: source.into(), label: label.into(),
    })).map(ShaderId::new)
}

pub fn create_sampler(mag_filter: FilterMode, min_filter: FilterMode) -> anyhow::Result<SamplerId> {
    create(resource_request::Command::CreateSampler(proto::CreateSampler {
        mag_filter: mag_filter as i32, min_filter: min_filter as i32,
    })).map(SamplerId::new)
}

/// View текстуры, выделенной через memory ABI (`texture_region` — region id).
pub fn create_texture_view(texture_region: u64) -> anyhow::Result<TextureViewId> {
    create(resource_request::Command::CreateTextureView(proto::CreateTextureView {
        texture_id: texture_region,
    })).map(TextureViewId::new)
}

pub fn create_bind_group_layout(label: &str, entries: Vec<BindGroupLayoutEntry>) -> anyhow::Result<BindGroupLayoutId> {
    create(resource_request::Command::CreateBindGroupLayout(proto::CreateBindGroupLayout {
        entries, label: label.into(),
    })).map(BindGroupLayoutId::new)
}

pub fn create_bind_group(label: &str, layout: &BindGroupLayoutId, entries: Vec<BindGroupEntry>) -> anyhow::Result<BindGroupId> {
    create(resource_request::Command::CreateBindGroup(proto::CreateBindGroup {
        layout_id: layout.id(), entries, label: label.into(),
    })).map(BindGroupId::new)
}

/// `desc.shader_id` и `desc.bind_group_layout_ids` заполняются из
/// соответствующих типизированных id (метод `id()`).
pub fn create_render_pipeline(desc: CreateRenderPipeline) -> anyhow::Result<PipelineId> {
    create(resource_request::Command::CreatePipeline(desc)).map(PipelineId::new)
}

// ── Конструкторы записей bind group ────────────────────────────

pub fn buffer_entry(binding: u32, buffer_region: u64) -> BindGroupEntry {
    BindGroupEntry { binding, resource: Some(proto::bind_group_entry::Resource::BufferId(buffer_region)) }
}

pub fn texture_entry(binding: u32, view: &TextureViewId) -> BindGroupEntry {
    BindGroupEntry { binding, resource: Some(proto::bind_group_entry::Resource::TextureViewId(view.id())) }
}

pub fn sampler_entry(binding: u32, sampler: &SamplerId) -> BindGroupEntry {
    BindGroupEntry { binding, resource: Some(proto::bind_group_entry::Resource::SamplerId(sampler.id())) }
}

/// Uniform-буфер вместе со своим layout'ом и bind group'ой — обряд один у
/// всех, кто отдаёт шейдеру блок констант. Буфер уходит во владельца до
/// создания bind group (сорвись она — буфер освободит Drop, а голый id повис
/// бы на хосте); layout возвращается вызывающему, потому что переживает bind
/// group — на него ссылаются пайплайны.
pub fn uniform_binding(
    label: &str,
    size: u64,
    visibility: u32,
) -> anyhow::Result<(crate::OwnedResource, BindGroupLayoutId, BindGroupId)> {
    let id = crate::abi::resource_alloc_buffer(size, buffer_usage::UNIFORM, false)
        .ok_or_else(|| anyhow::anyhow!("не выделился uniform-буфер ({}, {} байт)", label, size))?;
    let region = crate::OwnedResource::new(crate::ResourceHandle { id, size });
    let layout = create_bind_group_layout(
        &format!("{} BGL", label),
        vec![uniform_buffer_layout_entry(0, visibility)],
    )?;
    let group = create_bind_group(
        &format!("{} BG", label),
        &layout,
        vec![buffer_entry(0, region.id())],
    )?;
    Ok((region, layout, group))
}

/// Атрибуты, лежащие в вершине подряд, без зазоров: смещения выводятся из
/// форматов, локации — из порядка. Ровно так `#[repr(C)]` раскладывает
/// структуру из f32-полей, поэтому второй записи раскладки — таблицы ручных
/// смещений — не существует. `array_stride` вызывающий берёт из
/// `size_of::<Vertex>()`: несовпадение суммы с ним значит, что структура
/// разошлась со списком форматов.
pub fn packed_attributes(formats: &[VertexFormat]) -> Vec<VertexAttribute> {
    let mut offset = 0;
    formats
        .iter()
        .enumerate()
        .map(|(location, format)| {
            let attribute = VertexAttribute {
                format: *format as i32,
                offset,
                shader_location: location as u32,
            };
            offset += match format {
                VertexFormat::VtxFloat32 | VertexFormat::VtxUint32 => 4,
                VertexFormat::VtxFloat32x2 => 8,
                VertexFormat::VtxFloat32x3 => 12,
                VertexFormat::VtxFloat32x4 => 16,
            };
            attribute
        })
        .collect()
}

// ── Конструкторы записей layout ────────────────────────────────

pub fn uniform_buffer_layout_entry(binding: u32, visibility: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding, visibility,
        ty: Some(proto::bind_group_layout_entry::Ty::Buffer(proto::BufferBindingLayout {
            r#type: BufferBindingType::BufUniform as i32, has_dynamic_offset: false,
        })),
    }
}

pub fn texture_layout_entry(binding: u32, visibility: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding, visibility,
        ty: Some(proto::bind_group_layout_entry::Ty::Texture(proto::TextureBindingLayout {
            sample_type: TextureSampleType::SampleFloat as i32,
            view_dimension: TextureViewDimension::ViewD2 as i32,
            multisampled: false,
        })),
    }
}

pub fn sampler_layout_entry(binding: u32, visibility: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding, visibility,
        ty: Some(proto::bind_group_layout_entry::Ty::Sampler(proto::SamplerBindingLayout {
            r#type: SamplerBindingType::SampFiltering as i32,
        })),
    }
}
