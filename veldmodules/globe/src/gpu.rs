//! Ресурсы устройства и запись кадра.
//!
//! Поделены надвое по тому, от чего зависят: [`Device`] — геометрия, пайплайны
//! и буфер камеры, которым размер места безразличен; [`Target`] — view чужой
//! текстуры и буфер глубины под неё, которые размером и определяются. Владелец
//! перевыделяет место — пересобирается только второе.

use veldsdk::abi::{resource_alloc_texture, resource_write};
use veldsdk::graphics::{
    self as gfx, buffer_usage, texture_usage, BindGroupId, BindGroupLayoutId, CompareFunction,
    CreateRenderPipeline, CullMode, DepthStencilState, FilterMode, GrowingBuffer, IndexFormat,
    PipelineId, PrimitiveTopology, RenderRecorder, SamplerId, StepMode, TextureFormat,
    TextureViewId, VertexBufferLayout, VertexFormat, VISIBILITY_FRAGMENT, VISIBILITY_VERTEX,
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
/// `struct Camera` в globe.wgsl: матрица по столбцам, за ней позиция глаза
/// половинами, каждая выровненная на 16 байт, и размер кадра в пикселях.
///
/// Матрица **без переноса**: глаз стои́т в начале координат, а вычитают его в
/// шейдере, из позиции вершины. Иначе перенос — число порядка единицы — сам
/// уезжал бы сюда в `f32`, и вершина, сколь угодно точная, теряла бы всё в
/// умножении на него (см. `geodesy::parts`).
///
/// Размер кадра нужен лентам контуров: их ширина экранная, и перевести
/// пиксели в доли отсечения можно только зная, сколько их в кадре
/// (см. `vs_ribbon`).
#[repr(C)]
#[derive(Clone, Copy)]
struct CameraUniform {
    view_proj: Mat4,
    eye: World,
    _pad: f32,
    eye_low: World,
    _pad2: f32,
    viewport: [f32; 2],
    _pad3: [f32; 2],
}

/// Вершина ленты выделенного контура: точка поверхности с нормалью, её соседи
/// по обходу и сторона, в которую эту копию вершины отводит шейдер.
///
/// Соседи здесь потому, что направление ленты считается на экране, а не в мире:
/// в мире оно у одного и того же звена разное на разном приближении.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RibbonVertex {
    pub position: World,
    pub position_low: World,
    pub normal: World,
    /// Смещения к соседям, а не их позиции: у соседних вершин контура старшие
    /// половины совпадают, и разность их, посчитанная в шейдере, дала бы ноль
    /// там, где до соседа полсотни метров (см. `geodesy::offset`).
    pub prev: World,
    pub next: World,
    /// −1 — левый край ленты, +1 — правый.
    pub side: f32,
}

/// С чего начинаются буферы контуров. Экран найденного — это десятки снимков по
/// нескольку сотен вершин, то есть сотни килобайт, и буфер под них дорастает
/// сам; мегабайт с самого начала был бы не запасом, а просто неиспользуемой
/// видеопамятью в те разы, когда очерчено две строки.
const INITIAL_OUTLINE_BUFFER: u64 = 64 * 1024;

/// С чего начинается буфер варп-сеток наложений. У гранулы ячейка — несколько
/// сотен вершин, у растра покрупнее — тысячи, и буфер дорастает сам; четверти
/// мегабайта хватает первому кадру, чтобы не расти трижды подряд.
const INITIAL_OVERLAY_BUFFER: u64 = 256 * 1024;

/// Вершина варп-сетки наложения: точка мира, координата в текстуре носителя и
/// прозрачность своего слоя. Последняя одинакова у всех вершин наложения —
/// вершиной она едет потому, что все наложения лежат в одном буфере и рисуются
/// одним пайплайном (см. `overlay::patch`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayVertex {
    pub position: World,
    pub position_low: World,
    pub uv: [f32; 2],
    pub alpha: f32,
}

/// Собранные патчи наложений: общий вершинный буфер и отрисовки диапазонами —
/// по одной на носителя. Пересобирается не кадром, а сменой состава (см.
/// module.rs): камера двигает только uniform, мир патчей от неё не зависит.
pub struct OverlayBatch {
    vertices: GrowingBuffer,
    pub draws: Vec<(BindGroupId, std::ops::Range<u32>)>,
}

impl OverlayBatch {
    pub fn new() -> Self {
        Self {
            vertices: GrowingBuffer::new(
                buffer_usage::VERTEX,
                "вершины наложений",
                INITIAL_OVERLAY_BUFFER,
            ),
            draws: Vec::new(),
        }
    }

    /// Заливает пересобранные патчи. Пустой набор законен: наложения сняты
    /// или их тайлы ещё не приехали.
    pub fn fill(
        &mut self,
        vertices: &[OverlayVertex],
        draws: Vec<(BindGroupId, std::ops::Range<u32>)>,
    ) -> anyhow::Result<()> {
        self.vertices.write(vertices)?;
        self.draws = draws;
        Ok(())
    }
}

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
    /// Он же вполсилы — им рисуются невыбранные, пока выбран один.
    dimmed: PipelineId,
    /// Лента выделенного: своя вершина, своя вершинная функция.
    ribbon: PipelineId,
    /// Штриховка занятой области — ею помечено место снимка, который на шар
    /// ещё едет. Вершина у неё та же, что у линий, отличается она топологией и
    /// фрагментным шейдером.
    hatch: PipelineId,
    /// Наложения: варп-сетки с текстурой тайла-носителя.
    overlay: PipelineId,
    overlay_layout: BindGroupLayoutId,
    overlay_sampler: SamplerId,
    /// Формат таргета, под который собраны пайплайны. Сменится — пересобирать.
    pub format: i32,
    body_indices: std::ops::Range<u32>,
    grid_indices: std::ops::Range<u32>,
    /// Контуры: своя пара буферов, потому что меняются они с каждым ответом на
    /// поиск, а Земля строится один раз.
    outline_vertices: GrowingBuffer,
    outline_indices: GrowingBuffer,
    outline_lines: std::ops::Range<u32>,
    /// Лента выделенного — своя пара: вершина у неё другая.
    ribbon_vertices: GrowingBuffer,
    ribbon_indices: GrowingBuffer,
    ribbon_range: std::ops::Range<u32>,
    /// Штриховка занятых областей — тоже своя: вершина у неё общая с линиями,
    /// но рисуется она треугольниками, и делить с ними индексный буфер было бы
    /// нечем.
    hatch_vertices: GrowingBuffer,
    hatch_indices: GrowingBuffer,
    hatch_range: std::ops::Range<u32>,
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
        // Выбранный — от остальных тем же, а остальные от самих себя —
        // силой: пока выбран один, соседние приглушены (см. globe.wgsl).
        let outline = pipeline(
            "Globe Outline", &shader, "fs_outline", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let dimmed = pipeline(
            "Globe Outline Dim", &shader, "fs_outline_dim", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        // Штриховка занятой области: треугольники вместо линий, а всё
        // остальное — вершина и обращение с глубиной — как у контуров: лежит
        // она на той же высоте и о том же и говорит.
        let hatch = pipeline(
            "Globe Hatch", &shader, "fs_hatch", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, false, CompareFunction::CmpLessEqual,
        )?;
        // Лента: треугольники вместо линий и своя вершина, а глубина как у
        // линий — она лежит на той же высоте и о ней же говорит.
        let ribbon = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "Globe Ribbon".into(),
            vertex_entry: "vs_ribbon".into(),
            fragment_entry: "fs_ribbon".into(),
            target_format: format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<RibbonVertex>() as u64,
                step_mode: StepMode::StepVertex as i32,
                // Как поля RibbonVertex, смещения выводятся.
                attributes: gfx::packed_attributes(&[
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32,
                ]),
            }],
            bind_group_layout_ids: vec![camera_layout.id()],
            primitive_topology: PrimitiveTopology::TopologyTriangleList as i32,
            // Лента отводится в стороны на экране, и какая её сторона окажется
            // лицевой — зависит от того, с какой стороны на неё смотрят.
            cull_mode: CullMode::None as i32,
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT as i32,
                depth_write_enabled: false,
                depth_compare: CompareFunction::CmpLessEqual as i32,
            }),
            ..Default::default()
        })?;

        // Наложения: своя вершина (точка + UV) и привязка тайла, глубина как у
        // сетки — лежат на поверхности, спор за неё решает порядок отрисовки.
        let overlay_layout = gfx::create_bind_group_layout(
            "Overlay Tile BGL",
            vec![
                gfx::texture_layout_entry(0, VISIBILITY_FRAGMENT),
                gfx::sampler_layout_entry(1, VISIBILITY_FRAGMENT),
            ],
        )?;
        // Linear в обе стороны: уровень выбран по экранному пикселю, ужатие в
        // кадре не глубже двух крат, тайлы sRGB — усреднение честное.
        let overlay_sampler = gfx::create_sampler(FilterMode::FiltLinear, FilterMode::FiltLinear)?;
        let overlay = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "Globe Overlay".into(),
            vertex_entry: "vs_overlay".into(),
            fragment_entry: "fs_overlay".into(),
            target_format: format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                step_mode: StepMode::StepVertex as i32,
                attributes: gfx::packed_attributes(&[
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x3,
                    VertexFormat::VtxFloat32x2,
                    VertexFormat::VtxFloat32,
                ]),
            }],
            bind_group_layout_ids: vec![camera_layout.id(), overlay_layout.id()],
            primitive_topology: PrimitiveTopology::TopologyTriangleList as i32,
            cull_mode: CullMode::None as i32,
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT as i32,
                depth_write_enabled: false,
                depth_compare: CompareFunction::CmpLessEqual as i32,
            }),
            ..Default::default()
        })?;

        Ok(Self {
            vertices,
            indices,
            camera,
            camera_bind_group,
            _camera_layout: camera_layout,
            body,
            grid,
            outline,
            dimmed,
            ribbon,
            hatch,
            overlay,
            overlay_layout,
            overlay_sampler,
            format,
            body_indices: mesh.surface,
            grid_indices: mesh.grid,
            outline_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "вершины контуров", INITIAL_OUTLINE_BUFFER,
            ),
            outline_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "индексы контуров", INITIAL_OUTLINE_BUFFER,
            ),
            outline_lines: 0..0,
            ribbon_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "вершины ленты", INITIAL_OUTLINE_BUFFER,
            ),
            ribbon_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "индексы ленты", INITIAL_OUTLINE_BUFFER,
            ),
            ribbon_range: 0..0,
            hatch_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "вершины штриховки", INITIAL_OUTLINE_BUFFER,
            ),
            hatch_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "индексы штриховки", INITIAL_OUTLINE_BUFFER,
            ),
            hatch_range: 0..0,
        })
    }

    /// Заливает контуры в буферы. Набор заменяется целиком — таким он и
    /// приходит (см. `Outlines` в types.proto).
    ///
    /// Отрезки гасятся первым делом: не залившийся буфер оставляет за собой
    /// новый размер при старом содержимом, и оставшийся от прошлого набора
    /// отрезок рисовал бы по нему что придётся. Отказ здесь только логируется,
    /// кадр после него пишется дальше — и пустой набор в нём честнее мусора.
    pub fn set_outlines(&mut self, outlines: &Outlines) -> anyhow::Result<()> {
        self.outline_lines = 0..0;
        self.ribbon_range = 0..0;
        self.hatch_range = 0..0;
        self.outline_vertices.write(&outlines.vertices)?;
        self.outline_indices.write(&outlines.indices)?;
        self.outline_lines = 0..outlines.indices.len() as u32;
        self.ribbon_vertices.write(&outlines.ribbon)?;
        self.ribbon_indices.write(&outlines.ribbon_indices)?;
        self.ribbon_range = 0..outlines.ribbon_indices.len() as u32;
        self.hatch_vertices.write(&outlines.hatch)?;
        self.hatch_indices.write(&outlines.hatch_indices)?;
        self.hatch_range = 0..outlines.hatch_indices.len() as u32;
        Ok(())
    }

    /// Привязка тайла-носителя: его view и общий сэмплер.
    pub fn overlay_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Overlay Tile BG",
            &self.overlay_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.overlay_sampler)],
        )
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
pub fn render(
    device: &Device,
    target: &Target,
    camera: &Camera,
    overlays: &OverlayBatch,
) -> anyhow::Result<()> {
    let (eye, eye_low) = camera.eye_parts();
    let uniform = CameraUniform {
        view_proj: camera.view_projection(target.aspect()),
        eye,
        _pad: 0.0,
        eye_low,
        _pad2: 0.0,
        viewport: [target.width.max(1) as f32, target.height.max(1) as f32],
        _pad3: [0.0, 0.0],
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

    // Наложения — после тела (его глубина заслоняет дальнюю сторону), но до
    // сетки и контуров: линии обязаны читаться поверх снимка.
    if !overlays.draws.is_empty() {
        if let Some(buffer) = overlays.vertices.id() {
            recorder.set_pipeline(&device.overlay);
            recorder.set_bind_group(0, &device.camera_bind_group);
            recorder.set_vertex_buffer(0, buffer, 0, overlays.vertices.filled());
            for (bind, range) in &overlays.draws {
                recorder.set_bind_group(1, bind);
                recorder.draw(range.clone(), 0..1);
            }
            // Дальше рисуют Земля и линии — их буферы возвращаются на место.
            recorder.set_vertex_buffer(0, device.vertices.id(), 0, 0);
        }
    }

    recorder.set_pipeline(&device.grid);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.grid_indices.clone(), 0, 0..1);

    // Штриховка занятых областей — первой из того, что кладут контуры: она
    // лежит под ними и под лентой, потому что говорит о месте, а они о самом
    // снимке. Зато поверх сетки и поверх уже показанных наложений, и это не
    // случайность: место, которое снимок займёт, надо видеть и там, где шар
    // уже чем-то накрыт.
    if !device.hatch_range.is_empty()
        && let (Some(vertices), Some(indices)) =
            (device.hatch_vertices.id(), device.hatch_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.hatch_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.hatch_indices.filled());
        recorder.set_pipeline(&device.hatch);
        recorder.set_bind_group(0, &device.camera_bind_group);
        recorder.draw_indexed(device.hatch_range.clone(), 0, 0..1);
    }

    // Контуры последними: они поверх сетки. Глубину линии не пишут, поэтому
    // между собой они разбираются только очередью записи, и решает её порядок
    // здесь: лента выделенного пишется после линий и потому видна поверх
    // накрывших его соседей.
    if !device.outline_lines.is_empty()
        && let (Some(vertices), Some(indices)) =
            (device.outline_vertices.id(), device.outline_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.outline_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.outline_indices.filled());

        // Пока выделен один контур, остальные рисуются вполсилы: лента и так
        // заметна, но приглушённые соседи не спорят с ней за взгляд.
        let plain = match device.ribbon_range.is_empty() {
            true => &device.outline,
            false => &device.dimmed,
        };
        recorder.set_pipeline(plain);
        recorder.set_bind_group(0, &device.camera_bind_group);
        recorder.draw_indexed(device.outline_lines.clone(), 0, 0..1);
    }

    if !device.ribbon_range.is_empty()
        && let (Some(vertices), Some(indices)) =
            (device.ribbon_vertices.id(), device.ribbon_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.ribbon_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.ribbon_indices.filled());
        recorder.set_pipeline(&device.ribbon);
        recorder.set_bind_group(0, &device.camera_bind_group);
        recorder.draw_indexed(device.ribbon_range.clone(), 0, 0..1);
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

