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

const DEPTH_FORMAT: TextureFormat = TextureFormat::TexDepth32Float;

#[repr(C)]
#[derive(Clone, Copy)]
struct CameraUniform {
    view_proj: Mat4,
    eye: World,
    _pad: f32,
    eye_low: World,
    _pad2: f32,
    viewport: [f32; 2],
    phase: f32,
    ripple: f32,
}

#[derive(Clone, Copy, Default)]
pub struct Ripple {
    pub phase: f32,
    pub strength: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RibbonVertex {
    pub position: World,
    pub position_low: World,
    pub normal: World,
    pub prev: World,
    pub next: World,
    pub side: f32,
}

const INITIAL_OUTLINE_BUFFER: u64 = 64 * 1024;

const INITIAL_OVERLAY_BUFFER: u64 = 256 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayVertex {
    pub position: World,
    pub position_low: World,
    pub uv: [f32; 2],
    pub alpha: f32,
    pub ripple: f32,
}

pub struct Patch {
    pub bind: BindGroupId,
    pub segments: u32,
    pub base: u32,
}

pub struct OverlayBatch {
    vertices: GrowingBuffer,
    indices: GrowingBuffer,
    blocks: Vec<(u32, std::ops::Range<u32>)>,
    laid: Vec<u32>,
    pub draws: Vec<(BindGroupId, std::ops::Range<u32>, i32)>,
}

impl OverlayBatch {
    pub fn new() -> Self {
        Self {
            vertices: GrowingBuffer::new(
                buffer_usage::VERTEX,
                "overlay vertices",
                INITIAL_OVERLAY_BUFFER,
            ),
            indices: GrowingBuffer::new(
                buffer_usage::INDEX,
                "overlay indices",
                INITIAL_OVERLAY_BUFFER,
            ),
            blocks: Vec::new(),
            laid: Vec::new(),
            draws: Vec::new(),
        }
    }

    fn block(&mut self, segments: u32) -> std::ops::Range<u32> {
        if let Some((_, range)) = self.blocks.iter().find(|(at, _)| *at == segments) {
            return range.clone();
        }
        let (side, from) = (segments + 1, self.laid.len() as u32);
        self.laid.reserve((segments * segments * 6) as usize);
        for row in 0..segments {
            for col in 0..segments {
                let a = row * side + col;
                let (b, c, d) = (a + 1, a + side, a + side + 1);
                self.laid.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        let range = from..self.laid.len() as u32;
        self.blocks.push((segments, range.clone()));
        range
    }

    pub fn fill(
        &mut self,
        vertices: &[OverlayVertex],
        patches: &[Patch],
    ) -> anyhow::Result<()> {
        self.draws.clear();
        let laid = self.laid.len();
        let draws: Vec<(BindGroupId, std::ops::Range<u32>, i32)> = patches
            .iter()
            .map(|patch| (patch.bind.clone(), self.block(patch.segments), patch.base as i32))
            .collect();

        self.vertices.write(vertices)?;
        if self.laid.len() != laid || self.indices.id().is_none() {
            let laid = std::mem::take(&mut self.laid);
            let written = self.indices.write(&laid);
            self.laid = laid;
            written?;
        }
        self.draws = draws;
        Ok(())
    }
}

pub struct Device {
    vertices: OwnedResource,
    indices: OwnedResource,
    camera: OwnedResource,
    camera_bind_group: BindGroupId,
    _camera_layout: BindGroupLayoutId,
    body: PipelineId,
    grid: PipelineId,
    outline: PipelineId,
    dimmed: PipelineId,
    ribbon: PipelineId,
    hatch: PipelineId,
    overlay: PipelineId,
    overlay_layout: BindGroupLayoutId,
    overlay_sampler: SamplerId,
    pub format: i32,
    body_indices: std::ops::Range<u32>,
    grid_indices: std::ops::Range<u32>,
    outline_vertices: GrowingBuffer,
    outline_indices: GrowingBuffer,
    outline_lines: std::ops::Range<u32>,
    ribbon_vertices: GrowingBuffer,
    ribbon_indices: GrowingBuffer,
    ribbon_range: std::ops::Range<u32>,
    hatch_vertices: GrowingBuffer,
    hatch_indices: GrowingBuffer,
    hatch_range: std::ops::Range<u32>,
}

impl Device {
    pub fn create(format: i32) -> anyhow::Result<Self> {
        let mesh = Mesh::build();

        let vertices = gfx::upload("vertices", &mesh.vertices, buffer_usage::VERTEX)?;
        let indices = gfx::upload("indices", &mesh.indices, buffer_usage::INDEX)?;

        let (camera, camera_layout, camera_bind_group) = gfx::uniform_binding(
            "Globe Camera",
            std::mem::size_of::<CameraUniform>() as u64,
            VISIBILITY_VERTEX | VISIBILITY_FRAGMENT,
        )?;

        let shader = gfx::create_shader(include_str!("globe.wgsl"), "Globe Shader")?;

        let body = pipeline(
            "Globe Body", &shader, "fs_body", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, true, CompareFunction::CmpLess,
        )?;
        let grid = pipeline(
            "Globe Grid", &shader, "fs_grid", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let outline = pipeline(
            "Globe Outline", &shader, "fs_outline", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let dimmed = pipeline(
            "Globe Outline Dim", &shader, "fs_outline_dim", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let hatch = pipeline(
            "Globe Hatch", &shader, "fs_hatch", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, false, CompareFunction::CmpLessEqual,
        )?;
        let ribbon = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "Globe Ribbon".into(),
            vertex_entry: "vs_ribbon".into(),
            fragment_entry: "fs_ribbon".into(),
            target_format: format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<RibbonVertex>() as u64,
                step_mode: StepMode::StepVertex as i32,
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
            cull_mode: CullMode::None as i32,
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT as i32,
                depth_write_enabled: false,
                depth_compare: CompareFunction::CmpLessEqual as i32,
            }),
            ..Default::default()
        })?;

        let overlay_layout = gfx::create_bind_group_layout(
            "Overlay Tile BGL",
            vec![
                gfx::texture_layout_entry(0, VISIBILITY_FRAGMENT),
                gfx::sampler_layout_entry(1, VISIBILITY_FRAGMENT),
            ],
        )?;
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
                buffer_usage::VERTEX, "outline vertices", INITIAL_OUTLINE_BUFFER,
            ),
            outline_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "outline indices", INITIAL_OUTLINE_BUFFER,
            ),
            outline_lines: 0..0,
            ribbon_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "ribbon vertices", INITIAL_OUTLINE_BUFFER,
            ),
            ribbon_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "ribbon indices", INITIAL_OUTLINE_BUFFER,
            ),
            ribbon_range: 0..0,
            hatch_vertices: GrowingBuffer::new(
                buffer_usage::VERTEX, "hatch vertices", INITIAL_OUTLINE_BUFFER,
            ),
            hatch_indices: GrowingBuffer::new(
                buffer_usage::INDEX, "hatch indices", INITIAL_OUTLINE_BUFFER,
            ),
            hatch_range: 0..0,
        })
    }

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

    pub fn overlay_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Overlay Tile BG",
            &self.overlay_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.overlay_sampler)],
        )
    }
}

pub struct Target {
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    view: TextureViewId,
    _depth: OwnedResource,
    depth_view: TextureViewId,
}

impl Target {
    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }

    pub fn create(texture_id: u64, width: u32, height: u32) -> anyhow::Result<Self> {
        let view = gfx::create_texture_view(texture_id)?;

        let depth_id = resource_alloc_texture(
            width, height, DEPTH_FORMAT as i32, texture_usage::RENDER_ATTACHMENT,
        ).ok_or_else(|| anyhow!("failed to allocate the depth buffer {}x{}", width, height))?;
        let depth = OwnedResource::new(ResourceHandle { id: depth_id, size: 0 });
        let depth_view = gfx::create_texture_view(depth.id())?;

        Ok(Self { texture_id, width, height, view, _depth: depth, depth_view })
    }
}

fn camera_uniform(
    camera: &Camera,
    aspect: f32,
    size: (u32, u32),
    ripple: Ripple,
) -> CameraUniform {
    let (eye, eye_low) = camera.eye_parts();
    CameraUniform {
        view_proj: camera.view_projection(aspect),
        eye,
        _pad: 0.0,
        eye_low,
        _pad2: 0.0,
        viewport: [size.0.max(1) as f32, size.1.max(1) as f32],
        phase: ripple.phase,
        ripple: ripple.strength,
    }
}

pub fn render(
    device: &Device,
    target: &Target,
    camera: &Camera,
    overlays: &OverlayBatch,
    ripple: Ripple,
) -> anyhow::Result<()> {
    let uniform =
        camera_uniform(camera, target.aspect(), (target.width, target.height), ripple);
    resource_write(device.camera.id(), 0, gfx::bytes_of(std::slice::from_ref(&uniform)))?;

    let mut recorder = RenderRecorder::new();
    recorder.set_viewport(0.0, 0.0, target.width as f32, target.height as f32, 0.0, 1.0);
    recorder.set_vertex_buffer(0, device.vertices.id(), 0, 0);
    recorder.set_index_buffer(device.indices.id(), IndexFormat::IdxUint32, 0, 0);

    recorder.set_pipeline(&device.body);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.body_indices.clone(), 0, 0..1);

    if !overlays.draws.is_empty()
        && let (Some(vertices), Some(indices)) =
            (overlays.vertices.id(), overlays.indices.id())
    {
        recorder.set_pipeline(&device.overlay);
        recorder.set_bind_group(0, &device.camera_bind_group);
        recorder.set_vertex_buffer(0, vertices, 0, overlays.vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, overlays.indices.filled());
        for (bind, range, base) in &overlays.draws {
            recorder.set_bind_group(1, bind);
            recorder.draw_indexed(range.clone(), *base, 0..1);
        }
        recorder.set_vertex_buffer(0, device.vertices.id(), 0, 0);
        recorder.set_index_buffer(device.indices.id(), IndexFormat::IdxUint32, 0, 0);
    }

    recorder.set_pipeline(&device.grid);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.grid_indices.clone(), 0, 0..1);

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

    if !device.outline_lines.is_empty()
        && let (Some(vertices), Some(indices)) =
            (device.outline_vertices.id(), device.outline_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.outline_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.outline_indices.filled());

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
            attributes: gfx::packed_attributes(&[
                VertexFormat::VtxFloat32x3,
                VertexFormat::VtxFloat32x3,
                VertexFormat::VtxFloat32x3,
            ]),
        }],
        bind_group_layout_ids: vec![camera_layout.id()],
        primitive_topology: topology as i32,
        cull_mode: CullMode::None as i32,
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT as i32,
            depth_write_enabled: depth_write,
            depth_compare: depth_compare as i32,
        }),
        ..Default::default()
    })
}
