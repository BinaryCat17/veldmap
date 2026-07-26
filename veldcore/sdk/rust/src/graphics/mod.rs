//! Типизированная обёртка над графическим сервисом хоста.
//!
//! Буферы и текстуры выделяются через memory ABI (`abi::arena_alloc_*`)
//! и адресуются region id. Здесь создаются непрозрачные GPU-объекты
//! (шейдеры, пайплайны, view, сэмплеры, bind group'ы) — каждый со своим
//! типом идентификатора, чтобы их нельзя было перепутать, — и записываются
//! render-команды через [`RenderRecorder`].

mod recorder;
pub use recorder::RenderRecorder;

/// Контракт `veldmap.graphics` живёт в общем дереве протокола, здесь — только
/// псевдоним: обёртки ниже адресуют свои типы как `proto::*`.
pub use crate::proto::graphics as proto;

pub use proto::{
    TextureFormat, VertexFormat, IndexFormat, FilterMode, StepMode,
    PrimitiveTopology, BlendFactor, BlendOperation, FrontFace, CullMode,
    BufferBindingType, SamplerBindingType, TextureSampleType, TextureViewDimension,
    VertexAttribute, VertexBufferLayout, BlendComponent, BlendState,
    BindGroupLayoutEntry, BindGroupEntry, CreateRenderPipeline,
};

use prost::Message;
use proto::{resource_request, ResourceRequest};

// ── Типизированные идентификаторы GPU-объектов ─────────────────

/// Сырой id GPU-объекта. Drop освобождает объект на хосте: тот же путь,
/// что у memory-регионов (veld_memory_free умеет и то, и другое).
/// Bind group/пайплайн на хосте держат wgpu-ссылки на свои ресурсы,
/// поэтому раннее освобождение view/сэмплера/шейдера после создания
/// bind group/пайплайна безопасно.
#[derive(Debug, PartialEq, Eq, Hash)]
struct RawGpuId(u64);

impl Drop for RawGpuId {
    fn drop(&mut self) {
        crate::abi::arena_free(self.0);
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

/// Битовые флаги wgpu::BufferUsages для `abi::arena_alloc_buffer`.
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

/// Битовые флаги wgpu::TextureUsages для `abi::arena_alloc_texture`.
pub mod texture_usage {
    pub const COPY_SRC: u32 = 1 << 0;
    pub const COPY_DST: u32 = 1 << 1;
    pub const TEXTURE_BINDING: u32 = 1 << 2;
    pub const STORAGE_BINDING: u32 = 1 << 3;
    pub const RENDER_ATTACHMENT: u32 = 1 << 4;
}

// ── Создание ресурсов ──────────────────────────────────────────

fn create(command: resource_request::Command) -> anyhow::Result<u64> {
    let req = ResourceRequest { command: Some(command) };
    let res_bytes = crate::abi::graphics_create_resource(req.encode_to_vec())?;
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
