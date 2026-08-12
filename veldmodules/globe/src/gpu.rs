//! Ресурсы устройства и запись кадра.
//!
//! Поделены надвое по тому, от чего зависят: [`Device`] — геометрия, пайплайны
//! и буфер камеры, которым размер места безразличен; [`Target`] — view чужой
//! текстуры и буфер глубины под неё, которые размером и определяются. Владелец
//! перевыделяет место — пересобирается только второе.

use veldsdk::abi::{resource_alloc_texture, resource_write};
use veldsdk::graphics::{
    self as gfx, buffer_usage, texture_usage, BindGroupId, BindGroupLayoutId, CompareFunction,
    CreateRenderPipeline, CullMode, DepthStencilState, GrowingBuffer, IndexFormat, PipelineId,
    PrimitiveTopology, RenderRecorder, StepMode, TextureFormat, TextureViewId,
    VertexBufferLayout, VertexFormat, VISIBILITY_FRAGMENT, VISIBILITY_VERTEX,
};
use veldsdk::proto::core::ResourceHandle;
use veldsdk::OwnedResource;

use anyhow::anyhow;

use crate::module::camera::{Camera, Mat4};
use crate::module::geodesy::World;
use crate::module::mesh::{Mesh, Vertex};
use crate::module::outlines::Outlines;

/// Формат буфера глубины. Один на оба пайплайна и на саму текстуру — иначе
/// расхождение вскрылось бы только на первой отрисовке.
const DEPTH_FORMAT: TextureFormat = TextureFormat::TexDepth32Float;

/// То, что уезжает в шейдер каждый кадр. Раскладка обязана совпадать со
/// `struct Camera` в globe.wgsl: матрица по столбцам, за ней позиция глаза,
/// выровненная на 16 байт.
#[repr(C)]
#[derive(Clone, Copy)]
struct CameraUniform {
    view_proj: Mat4,
    eye: World,
    _pad: f32,
}

/// С чего начинаются буферы контуров. Экран найденного — это десятки снимков по
/// сотне вершин, то есть единицы килобайт; мегабайт под них был бы не запасом,
/// а просто неиспользуемой видеопамятью.
const INITIAL_OUTLINE_BUFFER: u64 = 64 * 1024;

pub struct Device {
    vertices: OwnedResource,
    indices: OwnedResource,
    camera: OwnedResource,
    camera_bind_group: BindGroupId,
    /// Layout переживает bind group: пайплайны сослались на него при сборке.
    _camera_layout: BindGroupLayoutId,
    body: PipelineId,
    grid: PipelineId,
    outline: PipelineId,
    picked: PipelineId,
    /// Формат таргета, под который собраны пайплайны. Сменится — пересобирать.
    pub format: i32,
    body_indices: std::ops::Range<u32>,
    grid_indices: std::ops::Range<u32>,
    /// Контуры: своя пара буферов, потому что меняются они с каждым ответом на
    /// поиск, а Земля строится один раз.
    outline_vertices: GrowingBuffer,
    outline_indices: GrowingBuffer,
    /// Куски индексного буфера под обычные контуры и под выделенные.
    outline_plain: std::ops::Range<u32>,
    outline_picked: std::ops::Range<u32>,
}

impl Device {
    /// Собирает всё, что не зависит от размера места. Геометрия строится здесь
    /// же и здесь же остаётся — на стороне хоста; повторно она не понадобится.
    pub fn create(format: i32) -> anyhow::Result<Self> {
        let mesh = Mesh::build();

        let vertices = gfx::upload("вершины", &mesh.vertices, buffer_usage::VERTEX)?;
        let indices = gfx::upload("индексы", &mesh.indices, buffer_usage::INDEX)?;

        let (camera, camera_layout, camera_bind_group) = gfx::uniform_binding(
            "Globe Camera",
            std::mem::size_of::<CameraUniform>() as u64,
            VISIBILITY_VERTEX | VISIBILITY_FRAGMENT,
        )?;

        let shader = gfx::create_shader(include_str!("globe.wgsl"), "Globe Shader")?;

        // Поверхность: пишет глубину — ею и закрывается всё, что за ней.
        let body = pipeline(
            "Globe Body", &shader, "fs_body", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, true, CompareFunction::CmpLess,
        )?;
        // Сетка приподнята над поверхностью, поэтому её ближняя половина тест
        // проходит, а дальняя — нет. Глубину не пишет: линия не должна
        // закрывать собой Землю, она на ней лежит.
        let grid = pipeline(
            "Globe Grid", &shader, "fs_grid", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        // Контуры устроены ровно как сетка и отличаются от неё только цветом.
        // Выделенный — от остальных тем же и ровно настолько же.
        let outline = pipeline(
            "Globe Outline", &shader, "fs_outline", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let picked = pipeline(
            "Globe Picked", &shader, "fs_picked", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;

        Ok(Self {
            vertices,
            indices,
            camera,
            camera_bind_group,
            _camera_layout: camera_layout,
            body,
            grid,
            outline,
            picked,
            format,
            body_indices: mesh.surface,
            grid_indices: mesh.grid,
            outline_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "вершины контуров", INITIAL_OUTLINE_BUFFER,
            ),
            outline_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "индексы контуров", INITIAL_OUTLINE_BUFFER,
            ),
            outline_plain: 0..0,
            outline_picked: 0..0,
        })
    }

    /// Заливает контуры в буферы. Набор заменяется целиком — таким он и
    /// приходит (см. `Outlines` в types.proto).
    pub fn set_outlines(&mut self, outlines: &Outlines) -> anyhow::Result<()> {
        self.outline_vertices.write(&outlines.vertices)?;
        self.outline_indices.write(&outlines.indices)?;
        self.outline_plain = outlines.plain.clone();
        self.outline_picked = outlines.picked.clone();
        Ok(())
    }
}

/// Место, отданное нам владельцем разметки, вместе с буфером глубины под него.
pub struct Target {
    /// Текстура не наша: её выделил и освободит владелец. Держим только id —
    /// по нему видно, что таргет сменился.
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    view: TextureViewId,
    /// Буфер глубины наш и приватный: наружу он не нужен никому.
    _depth: OwnedResource,
    depth_view: TextureViewId,
}

impl Target {
    /// Соотношение сторон места. Одна формула на проекцию и обратный луч
    /// (`Camera::ray`): посчитанные порознь, они сходились бы, только пока
    /// кто-то держит две копии одинаковыми.
    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }

    pub fn create(texture_id: u64, width: u32, height: u32) -> anyhow::Result<Self> {
        let view = gfx::create_texture_view(texture_id)?;

        let depth_id = resource_alloc_texture(
            width, height, DEPTH_FORMAT as i32, texture_usage::RENDER_ATTACHMENT,
        ).ok_or_else(|| anyhow!("не выделился буфер глубины {}x{}", width, height))?;
        let depth = OwnedResource::new(ResourceHandle { id: depth_id, size: 0 });
        let depth_view = gfx::create_texture_view(depth.id())?;

        Ok(Self { texture_id, width, height, view, _depth: depth, depth_view })
    }
}

/// Кадр: обновить камеру и записать отрисовку.
///
/// Исполнит записанное кадровый цикл хоста — здесь работа только ставится в
/// очередь на этот таргет.
pub fn render(device: &Device, target: &Target, camera: &Camera) -> anyhow::Result<()> {
    let uniform = CameraUniform {
        view_proj: camera.view_projection(target.aspect()),
        eye: camera.eye(),
        _pad: 0.0,
    };
    resource_write(device.camera.id(), 0, gfx::bytes_of(std::slice::from_ref(&uniform)))?;

    let mut recorder = RenderRecorder::new();
    recorder.set_viewport(0.0, 0.0, target.width as f32, target.height as f32, 0.0, 1.0);
    recorder.set_vertex_buffer(0, device.vertices.id(), 0, 0);
    recorder.set_index_buffer(device.indices.id(), IndexFormat::IdxUint32, 0, 0);

    // Bind group ставится после каждой смены пайплайна: у этих двоих layout
    // общий, и переживание им не запрещено, — но правило «сначала пайплайн,
    // потом его привязки» не зависит от того, совпали ли layout'ы.
    recorder.set_pipeline(&device.body);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.body_indices.clone(), 0, 0..1);

    recorder.set_pipeline(&device.grid);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.grid_indices.clone(), 0, 0..1);

    // Контуры последними: они поверх сетки. Глубину линии не пишут, поэтому
    // между собой они разбираются только очередью записи, и решает её порядок
    // здесь.
    if let (Some(vertices), Some(indices)) =
        (device.outline_vertices.id(), device.outline_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.outline_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.outline_indices.filled());

        for (pipeline, range) in [
            (&device.outline, &device.outline_plain),
            (&device.picked, &device.outline_picked),
        ] {
            if range.is_empty() {
                continue;
            }
            recorder.set_pipeline(pipeline);
            recorder.set_bind_group(0, &device.camera_bind_group);
            recorder.draw_indexed(range.clone(), 0, 0..1);
        }
    }

    recorder.submit_with_depth(&target.view, &target.depth_view)
}

/// Пайплайн отличается от соседнего четырьмя вещами, и ровно они здесь и
/// параметры: чем красить, чем рисовать и как обращаться с глубиной. Вершинная
/// функция, слой привязок и формат таргета у них общие.
fn pipeline(
    label: &str,
    shader: &gfx::ShaderId,
    fragment_entry: &str,
    camera_layout: &BindGroupLayoutId,
    format: i32,
    topology: PrimitiveTopology,
    depth_write: bool,
    depth_compare: CompareFunction,
) -> anyhow::Result<PipelineId> {
    gfx::create_render_pipeline(CreateRenderPipeline {
        shader_id: shader.id(),
        label: label.into(),
        vertex_entry: "vs_main".into(),
        fragment_entry: fragment_entry.into(),
        target_format: format,
        vertex_layouts: vec![VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: StepMode::StepVertex as i32,
            // Позиция и нормаль — как поля Vertex, смещения выводятся.
            attributes: gfx::packed_attributes(&[
                VertexFormat::VtxFloat32x3,
                VertexFormat::VtxFloat32x3,
            ]),
        }],
        bind_group_layout_ids: vec![camera_layout.id()],
        primitive_topology: topology as i32,
        // Отсечения нет: замкнутая поверхность и без него выглядит так же, а
        // решает задачу тест глубины — он же нужен и сетке, у которой лица не
        // разобрать.
        cull_mode: CullMode::None as i32,
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT as i32,
            depth_write_enabled: depth_write,
            depth_compare: depth_compare as i32,
        }),
        ..Default::default()
    })
}

