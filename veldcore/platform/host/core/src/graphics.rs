use std::sync::Arc;
use std::sync::Mutex;
use crate::format::{proto_to_wgpu, wgpu_to_proto};
use crate::memory::MemoryManager;
use crate::registry::{ResourceRegistry, ResourceId, ResourcePayload, GpuObject, Access};
use prost::Message;

/// Сгенерированные prost-типы протокола `veldmap.graphics`
/// (platform/host/generated, buildgen).
pub use veldmap_host_bindings::proto::graphics as proto;

use proto::{
    ResourceRequest, Submit, CommandBuffer,
    resource_request::Command as ResourceCommand,
    render_command::Command as RenderCommand,
    bind_group_layout_entry::Ty,
    VertexFormat, StepMode, FilterMode, PrimitiveTopology,
    FrontFace, CullMode,
    BufferBindingType, SamplerBindingType, TextureSampleType, TextureViewDimension,
};

// ── Render command queue ───────────────────────────────────────

/// Op в очереди держит Arc на всё, на что ссылается: аттачменты, пайплайн,
/// bind group'ы, буферы. Модули живут своими акторами и вправе освободить
/// объект следующим же сообщением — раньше, чем кадровый цикл доберётся до
/// очереди; кадр, собранный до освобождения, обязан дорисоваться тем, из чего
/// был собран. Разрешение id — только в submit (см. [`GraphicsDevice::execute`]),
/// пока объекты заведомо живы: сам модуль ничего освободить не может, его
/// вызов ещё не вернулся.
pub struct PendingRenderOp {
    pub attachments: Attachments,
    pub commands: Vec<ResolvedCommand>,
}

/// Команда рендера с уже разрешёнными объектами. Числа скопированы из
/// протокола как есть; клампы viewport и scissor остаются на исполнении —
/// они свойство таргета, а не команды.
pub enum ResolvedCommand {
    SetPipeline(Arc<wgpu::RenderPipeline>),
    SetBindGroup { index: u32, bind_group: Arc<wgpu::BindGroup> },
    SetVertexBuffer { slot: u32, buffer: Arc<wgpu::Buffer>, offset: u64, size: u64 },
    SetIndexBuffer { buffer: Arc<wgpu::Buffer>, offset: u64, size: u64, format: wgpu::IndexFormat },
    Draw { first_vertex: u32, vertex_count: u32, first_instance: u32, instance_count: u32 },
    DrawIndexed { first_index: u32, index_count: u32, base_vertex: i32, first_instance: u32, instance_count: u32 },
    SetViewport { x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32 },
    SetScissorRect { x: u32, y: u32, width: u32, height: u32 },
}

/// Разрешённые аттачменты одного render-op'а — всё, что нужно, чтобы открыть
/// pass. Собираются здесь, а не в раннере: правила, отличающие цветной таргет
/// от буфера глубины, живут рядом с самим предикатом (`format::is_depth`).
pub struct Attachments {
    pub target: Arc<wgpu::TextureView>,
    /// Размеры таргета — по ним кадровый цикл клампит viewport и scissor.
    pub width: u32,
    pub height: u32,
    pub depth: Option<Arc<wgpu::TextureView>>,
}

// ── GraphicsDevice ─────────────────────────────────────────────

pub struct GraphicsDevice {
    registry: Arc<ResourceRegistry>,
    memory: Arc<MemoryManager>,
    device: Arc<wgpu::Device>,
    surface_format: wgpu::TextureFormat,
    /// Кадровые команды, записанные модулями; дренирует их кадровый цикл
    /// раннера.
    pending_ops: Mutex<Vec<PendingRenderOp>>,
}

impl GraphicsDevice {
    /// Очереди здесь нет намеренно: writes в GPU идут через `MemoryManager`,
    /// у которого она своя. Второй handle сюда только раздваивал бы владельца
    /// одной и той же очереди.
    pub fn new(
        registry: Arc<ResourceRegistry>,
        memory: Arc<MemoryManager>,
        device: Arc<wgpu::Device>,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            registry,
            memory,
            device,
            surface_format,
            pending_ops: Mutex::new(Vec::new()),
        }
    }

    /// Забрать всё, что модули записали с прошлого кадра.
    pub fn take_pending_ops(&self) -> Vec<PendingRenderOp> {
        std::mem::take(&mut *self.pending_ops.lock().unwrap())
    }

    pub fn registry(&self) -> &Arc<ResourceRegistry> { &self.registry }
    pub fn memory(&self) -> &Arc<MemoryManager> { &self.memory }
    pub fn get_surface_format_proto(&self) -> i32 { wgpu_to_proto(self.surface_format) }

    // ── GPU object helpers ────────────────────────────────────
    // Носителей отдельно нет: GPU-объекты — такой же payload в реестре,
    // как и байтовые ресурсы; освобождение общее (registry.unregister,
    // см. veld_resource_free в abi.rs).

    fn insert_gpu(&self, obj: GpuObject, owner_id: u32) -> ResourceId {
        self.registry.register(ResourcePayload::Gpu(obj), owner_id)
    }

    /// GPU-объект по id с проверкой read-доступа. «Не найден» отвечаем и на
    /// байтовый ресурс: чужой тип носителя для вызывающего неразличим с
    /// отсутствующим id.
    pub fn get_gpu(&self, id: ResourceId, requestor_id: u32) -> anyhow::Result<GpuObject> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return Err(anyhow::anyhow!("Access denied to GPU object {}", id));
        }
        self.registry.payload(id, |p| match p {
            ResourcePayload::Gpu(obj) => Ok(obj.clone()),
            ResourcePayload::Data(_) => Err(anyhow::anyhow!("GPU object {} not found", id)),
        }).ok_or_else(|| anyhow::anyhow!("GPU object {} not found", id))?
    }

    // ── Lookup helpers ────────────────────────────────────────

    pub fn get_buffer(&self, id: ResourceId, requestor_id: u32) -> Option<Arc<wgpu::Buffer>> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return None;
        }
        self.memory.get_buffer(id)
    }

    pub fn get_texture_info(&self, id: ResourceId, requestor_id: u32) -> Option<(Arc<wgpu::Texture>, u32, u32, i32)> {
        if !self.registry.check_access(id, requestor_id, Access::Read) {
            return None;
        }
        self.memory.get_texture(id)
    }

    // ── GPU object creation ───────────────────────────────────

    pub fn create_texture_view(&self, texture_id: ResourceId, owner_id: u32) -> anyhow::Result<ResourceId> {
        let (texture, width, height, format) = self.get_texture_info(texture_id, owner_id)
            .ok_or_else(|| anyhow::anyhow!("Texture region {} not found or access denied", texture_id))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(self.insert_gpu(GpuObject::TextureView { view: Arc::new(view), width, height, format }, owner_id))
    }

    pub fn create_sampler(&self, mag_proto: i32, min_proto: i32, owner_id: u32) -> ResourceId {
        let map = |proto: i32| match FilterMode::try_from(proto).unwrap_or(FilterMode::FiltLinear) {
            FilterMode::FiltNearest => wgpu::FilterMode::Nearest,
            FilterMode::FiltLinear => wgpu::FilterMode::Linear,
        };
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: map(mag_proto), min_filter: map(min_proto), ..Default::default()
        });
        self.insert_gpu(GpuObject::Sampler(Arc::new(sampler)), owner_id)
    }

    pub fn create_bind_group_layout(&self, entries: &[wgpu::BindGroupLayoutEntry], owner_id: u32) -> ResourceId {
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries,
        });
        self.insert_gpu(GpuObject::BindGroupLayout(Arc::new(layout)), owner_id)
    }

    pub fn create_shader(&self, source: &str, label: Option<&str>, owner_id: u32) -> ResourceId {
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label, source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.insert_gpu(GpuObject::ShaderModule(Arc::new(shader)), owner_id)
    }

    pub fn create_bind_group(&self, layout_id: ResourceId, entries_proto: &[proto::BindGroupEntry], owner_id: u32) -> anyhow::Result<ResourceId> {
        let layout = match self.get_gpu(layout_id, owner_id)? {
            GpuObject::BindGroupLayout(l) => l,
            _ => return Err(anyhow::anyhow!("Object {} is not a BGL", layout_id)),
        };

        // Два прохода не случайны: wgpu::BindGroupEntry держит &-ссылки, а
        // ссылаться можно только на то, что переживёт вызов create_bind_group.
        // Первый проход собирает владеющие Arc'и в keep_*, второй строит
        // entries ссылками в них.
        let mut keep_buffers: Vec<(u32, Arc<wgpu::Buffer>)> = Vec::new();
        let mut keep_views: Vec<(u32, Arc<wgpu::TextureView>)> = Vec::new();
        let mut keep_samplers: Vec<(u32, Arc<wgpu::Sampler>)> = Vec::new();

        for e in entries_proto {
            match &e.resource {
                Some(proto::bind_group_entry::Resource::BufferId(bid)) => {
                    let b = self.get_buffer(*bid, owner_id)
                        .ok_or_else(|| anyhow::anyhow!("Buffer {} not found or access denied", bid))?;
                    keep_buffers.push((e.binding, b));
                }
                Some(proto::bind_group_entry::Resource::TextureViewId(tvid)) => {
                    match self.get_gpu(*tvid, owner_id) {
                        Ok(GpuObject::TextureView { view, .. }) => { keep_views.push((e.binding, view)); }
                        _ => {
                            if let Some((tex, _, _, _)) = self.get_texture_info(*tvid, owner_id) {
                                let v = Arc::new(tex.create_view(&wgpu::TextureViewDescriptor::default()));
                                keep_views.push((e.binding, v));
                            } else {
                                return Err(anyhow::anyhow!("TextureView {} not found or access denied", tvid));
                            }
                        }
                    }
                }
                Some(proto::bind_group_entry::Resource::SamplerId(sid)) => {
                    let s = match self.get_gpu(*sid, owner_id)? {
                        GpuObject::Sampler(s) => s,
                        _ => return Err(anyhow::anyhow!("Object {} is not a Sampler", sid)),
                    };
                    keep_samplers.push((e.binding, s));
                }
                None => {}
            }
        }

        let mut entries = Vec::new();
        for e in entries_proto {
            let resource = match &e.resource {
                Some(proto::bind_group_entry::Resource::BufferId(_)) => {
                    let b = &keep_buffers.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::Buffer(b.as_entire_buffer_binding())
                }
                Some(proto::bind_group_entry::Resource::TextureViewId(_)) => {
                    let tv = &keep_views.iter().find(|(binding, _)| *binding == e.binding).unwrap().1;
                    wgpu::BindingResource::TextureView(tv)
                }
                Some(proto::bind_group_entry::Resource::SamplerId(_)) => {
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

    pub fn create_pipeline(&self, req: &proto::CreateRenderPipeline, owner_id: u32) -> anyhow::Result<ResourceId> {
        let shader = match self.get_gpu(req.shader_id, owner_id)? {
            GpuObject::ShaderModule(s) => s,
            _ => return Err(anyhow::anyhow!("Object {} is not a Shader", req.shader_id)),
        };

        let target_format = proto_to_wgpu(req.target_format);

        let mut bgl_refs = Vec::new();
        for &id in &req.bind_group_layout_ids {
            match self.get_gpu(id, owner_id)? {
                GpuObject::BindGroupLayout(l) => bgl_refs.push(l),
                _ => return Err(anyhow::anyhow!("Object {} is not a BGL", id)),
            }
        }

        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            // Слоты layout'а необязательные: пропуск в середине списка теперь
            // выражается как None, а не сдвигом остальных.
            label: None, bind_group_layouts: &bgl_refs.iter().map(|l| Some(l.as_ref())).collect::<Vec<_>>(),
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

        // Смешивание одно на все пайплайны: обычная альфа поверх готового.
        // Настраивать его нечем и незачем — все, кто рисует, рисуют так.
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);

        // Формат проверяется здесь, а не отдаётся wgpu: у него это ошибка
        // валидации, а её обработчик по умолчанию роняет процесс — тогда как
        // модулю достаточно узнать, что пайплайн не собрался.
        let depth_stencil = match req.depth_stencil.as_ref() {
            Some(d) if !crate::format::is_depth(d.format) => {
                return Err(anyhow::anyhow!(
                    "depth_stencil names format {}, which is not a depth format", d.format));
            }
            // Трафарета и сдвига глубины в протоколе нет: ни того, ни другого
            // пока не просит ни один пайплайн, а завести их можно только вместе
            // с тем, кто ими пользуется.
            Some(d) => Some(wgpu::DepthStencilState {
                format: proto_to_wgpu(d.format),
                depth_write_enabled: Some(d.depth_write_enabled),
                depth_compare: Some(crate::format::compare_to_wgpu(d.depth_compare)),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            None => None,
        };

        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: if req.label.is_empty() { None } else { Some(&req.label) },
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some(&req.vertex_entry),
                buffers: &wgpu_vertex_layouts.iter().cloned().map(Some).collect::<Vec<_>>(),
                compilation_options: Default::default(),
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
                strip_index_format: None,
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
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(self.insert_gpu(GpuObject::RenderPipeline(Arc::new(pipeline)), owner_id))
    }

    // ── Create resource (protobuf dispatch) ───────────────────

    pub fn create_resource(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<u64> {
        let req = ResourceRequest::decode(&payload[..])?;

        let id = match req.command {
            Some(ResourceCommand::CreateShader(s)) => {
                let label = if s.label.is_empty() { None } else { Some(s.label.as_str()) };
                self.create_shader(&s.source, label, requestor_id)
            }
            Some(ResourceCommand::CreatePipeline(p)) => {
                self.create_pipeline(&p, requestor_id)?
            }
            Some(ResourceCommand::CreateSampler(s)) => {
                self.create_sampler(s.mag_filter, s.min_filter, requestor_id)
            }
            Some(ResourceCommand::CreateTextureView(tv)) => {
                self.create_texture_view(tv.texture_id, requestor_id)?
            }
            Some(ResourceCommand::CreateBindGroupLayout(bgl)) => {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = bgl.entries.iter().map(|e| {
                    let visibility = wgpu::ShaderStages::from_bits_truncate(e.visibility);
                    let ty = match &e.ty {
                        Some(Ty::Buffer(b)) => wgpu::BindingType::Buffer {
                            ty: match BufferBindingType::try_from(b.r#type).unwrap_or(BufferBindingType::BufUniform) {
                                BufferBindingType::BufUniform => wgpu::BufferBindingType::Uniform,
                                BufferBindingType::BufStorage => wgpu::BufferBindingType::Storage { read_only: false },
                                BufferBindingType::BufStorageReadonly => wgpu::BufferBindingType::Storage { read_only: true },
                            },
                            has_dynamic_offset: b.has_dynamic_offset, min_binding_size: None,
                        },
                        Some(Ty::Sampler(s)) => wgpu::BindingType::Sampler(
                            match SamplerBindingType::try_from(s.r#type).unwrap_or(SamplerBindingType::SampFiltering) {
                                SamplerBindingType::SampFiltering => wgpu::SamplerBindingType::Filtering,
                                SamplerBindingType::SampNonFiltering => wgpu::SamplerBindingType::NonFiltering,
                                SamplerBindingType::SampComparison => wgpu::SamplerBindingType::Comparison,
                            }
                        ),
                        Some(Ty::Texture(t)) => wgpu::BindingType::Texture {
                            sample_type: match TextureSampleType::try_from(t.sample_type).unwrap_or(TextureSampleType::SampleFloat) {
                                TextureSampleType::SampleFloat => wgpu::TextureSampleType::Float { filterable: true },
                                TextureSampleType::SampleUnfilterableFloat => wgpu::TextureSampleType::Float { filterable: false },
                                TextureSampleType::SampleDepth => wgpu::TextureSampleType::Depth,
                                TextureSampleType::SampleUint => wgpu::TextureSampleType::Uint,
                                TextureSampleType::SampleSint => wgpu::TextureSampleType::Sint,
                            },
                            view_dimension: match TextureViewDimension::try_from(t.view_dimension).unwrap_or(TextureViewDimension::ViewD2) {
                                TextureViewDimension::ViewD1 => wgpu::TextureViewDimension::D1,
                                TextureViewDimension::ViewD2 => wgpu::TextureViewDimension::D2,
                                TextureViewDimension::ViewD2Array => wgpu::TextureViewDimension::D2Array,
                                TextureViewDimension::ViewCube => wgpu::TextureViewDimension::Cube,
                                TextureViewDimension::ViewCubeArray => wgpu::TextureViewDimension::CubeArray,
                                TextureViewDimension::ViewD3 => wgpu::TextureViewDimension::D3,
                            },
                            multisampled: t.multisampled,
                        },
                        None => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None,
                        },
                    };
                    wgpu::BindGroupLayoutEntry { binding: e.binding, visibility, ty, count: None }
                }).collect();
                self.create_bind_group_layout(&entries, requestor_id)
            }
            Some(ResourceCommand::CreateBindGroup(bg)) => {
                self.create_bind_group(bg.layout_id, &bg.entries, requestor_id)?
            }
            None => return Err(anyhow::anyhow!("Empty resource request")),
        };
        Ok(id)
    }

    // ── Execute (queue render commands for the frame loop) ────

    /// Буфер глубины требует того же права, что и цветной таргет: pass его
    /// очищает и в него пишет, а это запись.
    ///
    /// Разрешение всех id — здесь же, синхронно с вызовом модуля: ошибка
    /// уходит ему ответом на submit, то есть туда, где ошиблись, а не warn'ом
    /// кадрового цикла спустя произвольное число сообщений.
    pub fn execute(&self, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<()> {
        let req = Submit::decode(&payload[..])?;
        if !self.registry.check_access(req.target_texture_view_id, requestor_id, Access::Write) {
            return Err(anyhow::anyhow!("Access denied to target view {}", req.target_texture_view_id));
        }
        if req.depth_texture_view_id != 0
            && !self.registry.check_access(req.depth_texture_view_id, requestor_id, Access::Write)
        {
            return Err(anyhow::anyhow!("Access denied to depth view {}", req.depth_texture_view_id));
        }
        if let Some(cb) = req.command_buffer {
            let op = PendingRenderOp {
                attachments: self.resolve_attachments(
                    req.target_texture_view_id,
                    req.depth_texture_view_id,
                    requestor_id,
                )?,
                commands: self.resolve_commands(&cb, requestor_id)?,
            };
            self.pending_ops.lock().unwrap().push(op);
        }
        Ok(())
    }

    /// Команды протокола → команды с живыми объектами. Валидная
    /// последовательность начинается с пайплайна: draw без него — ошибка
    /// валидации wgpu, а её обработчик по умолчанию роняет процесс.
    fn resolve_commands(
        &self,
        command_buffer: &CommandBuffer,
        requestor_id: u32,
    ) -> anyhow::Result<Vec<ResolvedCommand>> {
        let buffer = |id: u64| {
            self.get_buffer(id, requestor_id)
                .ok_or_else(|| anyhow::anyhow!("объект {} не буфер либо недоступен", id))
        };

        let mut resolved = Vec::with_capacity(command_buffer.commands.len());
        let mut pipeline_bound = false;
        for render_cmd in &command_buffer.commands {
            let cmd = match &render_cmd.command { Some(c) => c, None => continue };
            resolved.push(match cmd {
                RenderCommand::SetPipeline(p) => match self.get_gpu(p.pipeline_id, requestor_id) {
                    Ok(GpuObject::RenderPipeline(pipeline)) => {
                        pipeline_bound = true;
                        ResolvedCommand::SetPipeline(pipeline)
                    }
                    _ => anyhow::bail!("объект {} не pipeline либо недоступен", p.pipeline_id),
                },
                RenderCommand::SetBindGroup(bg) => match self.get_gpu(bg.bind_group_id, requestor_id) {
                    Ok(GpuObject::BindGroup(bind_group)) => {
                        ResolvedCommand::SetBindGroup { index: bg.index, bind_group }
                    }
                    _ => anyhow::bail!("объект {} не bind group либо недоступен", bg.bind_group_id),
                },
                RenderCommand::SetVertexBuffer(vb) => ResolvedCommand::SetVertexBuffer {
                    slot: vb.slot,
                    buffer: buffer(vb.buffer_id)?,
                    offset: vb.offset,
                    size: vb.size,
                },
                RenderCommand::SetIndexBuffer(ib) => ResolvedCommand::SetIndexBuffer {
                    buffer: buffer(ib.buffer_id)?,
                    offset: ib.offset,
                    size: ib.size,
                    format: if ib.index_format == 1 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 },
                },
                RenderCommand::Draw(d) => {
                    if !pipeline_bound {
                        anyhow::bail!("draw до первого SetPipeline");
                    }
                    ResolvedCommand::Draw {
                        first_vertex: d.first_vertex,
                        vertex_count: d.vertex_count,
                        first_instance: d.first_instance,
                        instance_count: d.instance_count,
                    }
                }
                RenderCommand::DrawIndexed(di) => {
                    if !pipeline_bound {
                        anyhow::bail!("draw до первого SetPipeline");
                    }
                    ResolvedCommand::DrawIndexed {
                        first_index: di.first_index,
                        index_count: di.index_count,
                        base_vertex: di.base_vertex,
                        first_instance: di.first_instance,
                        instance_count: di.instance_count,
                    }
                }
                RenderCommand::SetViewport(v) => ResolvedCommand::SetViewport {
                    x: v.x, y: v.y, width: v.width, height: v.height,
                    min_depth: v.min_depth, max_depth: v.max_depth,
                },
                RenderCommand::SetScissorRect(s) => ResolvedCommand::SetScissorRect {
                    x: s.x, y: s.y, width: s.width, height: s.height,
                },
            });
        }
        Ok(resolved)
    }

    /// Разрешает аттачменты op'а и проверяет то, что wgpu проверил бы сам —
    /// но ошибкой валидации, роняющей процесс: роль текстуры (цвет или
    /// глубина) и совпадение размеров.
    fn resolve_attachments(
        &self,
        target_view_id: u64,
        depth_view_id: u64,
        requestor_id: u32,
    ) -> anyhow::Result<Attachments> {
        let view = |id: u64| match self.get_gpu(id, requestor_id) {
            Ok(GpuObject::TextureView { view, width, height, format }) => Ok((view, width, height, format)),
            _ => Err(anyhow::anyhow!("view {} is unknown or not a texture view", id)),
        };

        let (target, width, height, format) = view(target_view_id)?;
        if crate::format::is_depth(format) {
            return Err(anyhow::anyhow!(
                "view {} is a depth buffer and cannot be a colour target", target_view_id));
        }

        let depth = match depth_view_id {
            0 => None,
            id => {
                let (depth_view, dw, dh, dformat) = view(id)?;
                if !crate::format::is_depth(dformat) {
                    return Err(anyhow::anyhow!("view {} is not a depth buffer", id));
                }
                if (dw, dh) != (width, height) {
                    return Err(anyhow::anyhow!(
                        "depth buffer {} is {}x{}, target is {}x{}", id, dw, dh, width, height));
                }
                Some(depth_view)
            }
        };

        Ok(Attachments { target, width, height, depth })
    }
}

// ── Render command execution ───────────────────────────────────

/// Кодирует разрешённый op в открытый pass. Ошибок здесь не бывает: всё, что
/// могло не найтись, не нашлось бы ещё на submit, а объекты живы — их держит
/// сам op. Клампы viewport и scissor — по размерам таргета: они свойство
/// аттачмента, каким он был на submit, и приходят вместе с ним.
pub fn execute_render_commands(
    rp: &mut wgpu::RenderPass<'_>,
    commands: &[ResolvedCommand],
    target_width: u32,
    target_height: u32,
) {
    for cmd in commands {
        match cmd {
            ResolvedCommand::SetPipeline(pipeline) => rp.set_pipeline(pipeline.as_ref()),
            ResolvedCommand::SetBindGroup { index, bind_group } => {
                rp.set_bind_group(*index, bind_group.as_ref(), &[]);
            }
            ResolvedCommand::SetVertexBuffer { slot, buffer, offset, size } => {
                let end = if *size > 0 { (offset + size).min(buffer.size()) } else { buffer.size() };
                rp.set_vertex_buffer(*slot, buffer.slice(*offset..end));
            }
            ResolvedCommand::SetIndexBuffer { buffer, offset, size, format } => {
                let end = if *size > 0 { (offset + size).min(buffer.size()) } else { buffer.size() };
                rp.set_index_buffer(buffer.slice(*offset..end), *format);
            }
            ResolvedCommand::Draw { first_vertex, vertex_count, first_instance, instance_count } => {
                rp.draw(
                    *first_vertex..(first_vertex + vertex_count),
                    *first_instance..(first_instance + instance_count),
                );
            }
            ResolvedCommand::DrawIndexed { first_index, index_count, base_vertex, first_instance, instance_count } => {
                rp.draw_indexed(
                    *first_index..(first_index + index_count),
                    *base_vertex,
                    *first_instance..(first_instance + instance_count),
                );
            }
            ResolvedCommand::SetViewport { x, y, width, height, min_depth, max_depth } => {
                let x = x.clamp(0.0, target_width as f32);
                let y = y.clamp(0.0, target_height as f32);
                let w = width.min(target_width as f32 - x);
                let h = height.min(target_height as f32 - y);
                if w > 0.0 && h > 0.0 { rp.set_viewport(x, y, w, h, *min_depth, *max_depth); }
            }
            ResolvedCommand::SetScissorRect { x, y, width, height } => {
                let x = (*x).min(target_width.saturating_sub(1));
                let y = (*y).min(target_height.saturating_sub(1));
                let w = (*width).min(target_width - x).max(1);
                let h = (*height).min(target_height - y).max(1);
                rp.set_scissor_rect(x, y, w, h);
            }
        }
    }
}
