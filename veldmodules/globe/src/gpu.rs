//! Device resources and frame recording.
//!
//! Split in two by what they depend on: [`Device`] — geometry, pipelines and
//! the camera buffer, which the target size does not affect; [`Target`] — a view
//! of someone else's texture and the depth buffer for it, which the size does
//! determine. The owner reallocates the target — only the second is rebuilt.

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

/// Depth buffer format. One for both pipelines and the texture itself —
/// otherwise the mismatch would surface only at the first draw.
const DEPTH_FORMAT: TextureFormat = TextureFormat::TexDepth32Float;

/// What goes to the shader every frame. The layout must match `struct Camera`
/// in globe.wgsl: the matrix column by column, then the eye position in halves,
/// each aligned to 16 bytes, and the frame size in pixels.
///
/// The matrix is **translation-free**: the eye sits at the origin and the
/// shader subtracts it from the vertex position. Otherwise the translation — a
/// number of order one — would travel here in `f32`, and a vertex, however
/// precise, would lose everything in multiplying by it (see `geodesy::parts`).
///
/// The frame size is needed by the outline ribbons: their width is in screen
/// pixels, and pixels convert to clip fractions only knowing how many of them
/// are in the frame (see `vs_ribbon`).
#[repr(C)]
#[derive(Clone, Copy)]
struct CameraUniform {
    view_proj: Mat4,
    eye: World,
    _pad: f32,
    eye_low: World,
    _pad2: f32,
    viewport: [f32; 2],
    /// Ripple phase as a fraction of its period, 0..1, and its strength, 0..1 —
    /// what marks the still-loading place of the image (see `fs_overlay`).
    ///
    /// The strength lives here, not in the vertex, because it is a property of
    /// the layer, not of the cell: put into the vertex, it would call for
    /// rebuilding the whole warp mesh on every movement of the read bytes. The
    /// cell carries only a flag — whether it ripples or not.
    phase: f32,
    ripple: f32,
}

/// Ripple on loading cells: where the wave stands now and how visible it is.
///
/// Both values at once, because separately they are meaningless: phase without
/// strength moves something invisible, strength without phase draws a standing
/// patch. The frame tick computes them, not the drawing: phase is measured in
/// time, and the drawing has no time.
#[derive(Clone, Copy, Default)]
pub struct Ripple {
    /// Fraction of the wave period, 0..1.
    pub phase: f32,
    /// Strength, 0..1: the closer the step is to its end, the more visible.
    /// Zero — nothing to ripple, and then the frame is no different from the
    /// previous one at all.
    pub strength: f32,
}

/// Vertex of the selected outline's ribbon: a surface point with a normal, its
/// neighbors along the traversal, and the side the shader shifts this copy of
/// the vertex toward.
///
/// The neighbors are here because the ribbon direction is computed on screen,
/// not in the world: in the world the same link has a different one at
/// different zoom levels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RibbonVertex {
    pub position: World,
    pub position_low: World,
    pub normal: World,
    /// Offsets to the neighbors, not their positions: adjacent outline vertices
    /// share the high halves, and their difference computed in the shader would
    /// be zero where the neighbor is fifty meters away (see `geodesy::offset`).
    pub prev: World,
    pub next: World,
    /// −1 — the left edge of the ribbon, +1 — the right one.
    pub side: f32,
}

/// What the outline buffers start from. A screen of findings is dozens of
/// images with a few hundred vertices each, that is hundreds of kilobytes, and
/// the buffer grows to fit them on its own; a megabyte from the start would be
/// not a reserve but simply unused video memory in the many cases when only two
/// rows are outlined.
const INITIAL_OUTLINE_BUFFER: u64 = 64 * 1024;

/// What the overlay warp mesh buffers start from — both vertices and indices. A
/// granule cell is some eighty nodes and four hundred indices, a raster is over
/// a thousand of each, and the buffers grow on their own; a quarter of a
/// megabyte lasts the first frame without growing three times in a row.
const INITIAL_OVERLAY_BUFFER: u64 = 256 * 1024;

/// Vertex of an overlay warp mesh: a world point, a coordinate in the carrier
/// texture, the opacity of its own layer, and a loading flag. The last two are
/// the same for all vertices of one cell — they travel as vertex attributes
/// because all overlays lie in one buffer and are drawn by one pipeline (see
/// `overlay::spread`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayVertex {
    pub position: World,
    pub position_low: World,
    pub uv: [f32; 2],
    pub alpha: f32,
    /// 1 — the cell's own tile is still to travel and the place under it
    /// ripples; 0 — the cell is covered by what it should be. A flag, not a
    /// strength: the frame's uniform carries the strength (see
    /// [`CameraUniform::ripple`]).
    pub ripple: f32,
}

/// One overlay draw: what to paint with, how dense its warp mesh is, and which
/// node of the shared buffer it starts from.
///
/// By density, not by index range: the indices of all cells of one density are
/// the same, and only the start distinguishes them (see
/// [`OverlayBatch::block`]).
pub struct Patch {
    pub bind: BindGroupId,
    pub segments: u32,
    pub base: u32,
}

/// The assembled overlay patches: a shared vertex buffer, an index buffer in
/// blocks by density, and draws — one per cell. Rebuilt not per frame but on
/// composition change (see module.rs): the camera moves only the uniform, the
/// patch set does not depend on it.
pub struct OverlayBatch {
    vertices: GrowingBuffer,
    indices: GrowingBuffer,
    /// Indices by density: where in the buffer the mesh block of `segments`
    /// segments lies. The block is built once per density and lives as long as
    /// the device — nothing in it depends on the cell.
    blocks: Vec<(u32, std::ops::Range<u32>)>,
    /// The indices themselves, to append a block. They are kept here because the
    /// buffer is filled whole: appending a tail to it without the head is
    /// impossible.
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

    /// Mesh indices of that density: taken from the blocks or built and put
    /// there.
    ///
    /// The quad's diagonal runs from `b` to `c`, and both triples of the six
    /// hold it: the drawn surface divides itself along it, the chord sag
    /// (`mesh::lift_m`) is computed along it too, and drawn differently it
    /// would disagree with the lift measure.
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

    /// Fills the rebuilt patches. An empty set is legal: the overlays are taken
    /// off or their tiles have not arrived yet.
    ///
    /// The draws are cleared first — for the same reason as with the outlines
    /// (see [`Device::set_outlines`]): a buffer that failed to fill keeps the
    /// new size with the old contents, and a range from the previous build
    /// would run past its end. There are two buffers here, and they can fall
    /// out of step with each other: the vertices filled, the indices did not.
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
        // The indices are filled only when a block was added: densities repeat
        // within a frame, and a routine rebuild does not touch them at all.
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
    /// The layout outlives the bind group: the pipelines referenced it at build.
    _camera_layout: BindGroupLayoutId,
    body: PipelineId,
    grid: PipelineId,
    outline: PipelineId,
    /// Also the half-strength one — it draws the unselected outlines while one
    /// is selected.
    dimmed: PipelineId,
    /// The selected outline's ribbon: its own vertex, its own vertex function.
    ribbon: PipelineId,
    /// Hatching of the occupied area — it marks the place of an image that is
    /// still traveling to the globe. Its vertex is the same as the lines', it
    /// differs in topology and the fragment shader.
    hatch: PipelineId,
    /// Overlays: warp meshes with the carrier tile's texture.
    overlay: PipelineId,
    overlay_layout: BindGroupLayoutId,
    overlay_sampler: SamplerId,
    /// The target format the pipelines are built for. Changes — rebuild them.
    pub format: i32,
    body_indices: std::ops::Range<u32>,
    grid_indices: std::ops::Range<u32>,
    /// Outlines: their own pair of buffers, because they change with every
    /// search response, while the Earth is built once.
    outline_vertices: GrowingBuffer,
    outline_indices: GrowingBuffer,
    outline_lines: std::ops::Range<u32>,
    /// The selected outline's ribbon — its own pair: its vertex is different.
    ribbon_vertices: GrowingBuffer,
    ribbon_indices: GrowingBuffer,
    ribbon_range: std::ops::Range<u32>,
    /// Hatching of occupied areas — its own pair too: its vertex is shared with
    /// the lines, but it is drawn with triangles, and the index buffer cannot
    /// be shared with them.
    hatch_vertices: GrowingBuffer,
    hatch_indices: GrowingBuffer,
    hatch_range: std::ops::Range<u32>,
}

impl Device {
    /// Builds everything that does not depend on the target size. The geometry
    /// is built here and stays here — on the host side; it will not be needed
    /// again.
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

        // The surface: it writes depth — with it everything behind is closed
        // off.
        let body = pipeline(
            "Globe Body", &shader, "fs_body", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, true, CompareFunction::CmpLess,
        )?;
        // The grid is lifted above the surface, so its near half passes the
        // depth test and the far half does not. It does not write depth: a line
        // must not close off the Earth, it lies on it.
        let grid = pipeline(
            "Globe Grid", &shader, "fs_grid", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        // Outlines are arranged exactly like the grid and differ from it only
        // in color. The selected one differs from the rest the same way, and
        // the rest from themselves — by strength: while one is selected, its
        // neighbors are dimmed (see globe.wgsl).
        let outline = pipeline(
            "Globe Outline", &shader, "fs_outline", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        let dimmed = pipeline(
            "Globe Outline Dim", &shader, "fs_outline_dim", &camera_layout, format,
            PrimitiveTopology::TopologyLineList, false, CompareFunction::CmpLessEqual,
        )?;
        // Hatching of the occupied area: triangles instead of lines, and
        // everything else — the vertex and the depth handling — as with the
        // outlines: it lies at the same height and speaks of the same thing.
        let hatch = pipeline(
            "Globe Hatch", &shader, "fs_hatch", &camera_layout, format,
            PrimitiveTopology::TopologyTriangleList, false, CompareFunction::CmpLessEqual,
        )?;
        // The ribbon: triangles instead of lines and its own vertex, with depth
        // as with the lines — it lies at the same height and speaks of the same.
        let ribbon = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "Globe Ribbon".into(),
            vertex_entry: "vs_ribbon".into(),
            fragment_entry: "fs_ribbon".into(),
            target_format: format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<RibbonVertex>() as u64,
                step_mode: StepMode::StepVertex as i32,
                // The offsets are derived like the RibbonVertex fields.
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
            // The ribbon is spread sideways on screen, and which of its sides
            // turns out front-facing depends on which side it is viewed from.
            cull_mode: CullMode::None as i32,
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT as i32,
                depth_write_enabled: false,
                depth_compare: CompareFunction::CmpLessEqual as i32,
            }),
            ..Default::default()
        })?;

        // Overlays: their own vertex (point + UV) and a tile binding, depth as
        // with the grid — they lie on the surface, the draw order settles the
        // contest over it.
        let overlay_layout = gfx::create_bind_group_layout(
            "Overlay Tile BGL",
            vec![
                gfx::texture_layout_entry(0, VISIBILITY_FRAGMENT),
                gfx::sampler_layout_entry(1, VISIBILITY_FRAGMENT),
            ],
        )?;
        // Linear in both directions: the level is chosen by the screen pixel,
        // the in-frame shrink is no deeper than twofold, the tiles are sRGB —
        // the averaging is honest.
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

    /// Fills the outlines into the buffers. The set is replaced whole — that is
    /// how it arrives (see `Outlines` in types.proto).
    ///
    /// The ranges are cleared first: a buffer that failed to fill keeps the new
    /// size with the old contents, and a range left from the previous set would
    /// draw whatever is lying there. A failure here is only logged, the frame
    /// after it is recorded further — and an empty set in it is more honest
    /// than garbage.
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

    /// The carrier tile binding: its view and the shared sampler.
    pub fn overlay_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Overlay Tile BG",
            &self.overlay_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.overlay_sampler)],
        )
    }
}

/// The space handed to us by the layout owner, together with the depth buffer
/// under it.
pub struct Target {
    /// The texture is not ours: the owner allocated it and will free it. We
    /// keep only the id — it shows that the target has changed.
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    view: TextureViewId,
    /// The depth buffer is ours and private: nobody outside needs it.
    _depth: OwnedResource,
    depth_view: TextureViewId,
}

impl Target {
    /// Aspect ratio of the target. One formula for the projection and the
    /// inverse ray (`Camera::ray`): computed separately, they would agree only
    /// while someone keeps the two copies identical.
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

/// What goes to the shader about the camera.
///
/// Separate from the drawing, because otherwise it is unverifiable: both halves
/// of the eye cannot be seen by eye — zero the low one and the picture stays
/// the same, only the vertices start stepping by a meter (see `geodesy::parts`).
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

/// Frame: update the camera and record the drawing.
///
/// The host's frame loop executes what is recorded — here the work is only
/// queued for this target.
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

    // The bind group is set after every pipeline change: these two share the
    // layout, and outliving it is not forbidden — but the rule "pipeline first,
    // then its bindings" does not depend on whether the layouts match.
    recorder.set_pipeline(&device.body);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.body_indices.clone(), 0, 0..1);

    // Overlays — after the body (its depth hides the far side) but before the
    // grid and the outlines: the lines must read on top of the image.
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
            // The indices are shared by all cells of one density, but each has
            // its own start, and `base_vertex` carries it: it is added to the
            // index already on the GPU.
            recorder.draw_indexed(range.clone(), *base, 0..1);
        }
        // Next the Earth and the lines are drawn — their buffers are restored.
        // Both of them: the grid is drawn right after the overlays with
        // indices, and a foreign index buffer left behind would drag it into
        // empty space.
        recorder.set_vertex_buffer(0, device.vertices.id(), 0, 0);
        recorder.set_index_buffer(device.indices.id(), IndexFormat::IdxUint32, 0, 0);
    }

    recorder.set_pipeline(&device.grid);
    recorder.set_bind_group(0, &device.camera_bind_group);
    recorder.draw_indexed(device.grid_indices.clone(), 0, 0..1);

    // Hatching of occupied areas goes first among what the outlines lay down:
    // it lies under them and under the ribbon, because it speaks of the place
    // while they speak of the image itself. But above the grid and above the
    // already shown overlays, and this is no accident: the place the image will
    // occupy must be visible even where the globe is already covered.
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

    // Outlines last: they are above the grid. Lines do not write depth, so
    // among themselves they are settled only by the record queue, and its order
    // is decided here: the selected outline's ribbon is recorded after the
    // lines and therefore is seen above the neighbors that cover it.
    if !device.outline_lines.is_empty()
        && let (Some(vertices), Some(indices)) =
            (device.outline_vertices.id(), device.outline_indices.id())
    {
        recorder.set_vertex_buffer(0, vertices, 0, device.outline_vertices.filled());
        recorder.set_index_buffer(indices, IndexFormat::IdxUint32, 0, device.outline_indices.filled());

        // While one outline is selected, the rest are drawn at half strength:
        // the ribbon is noticeable anyway, and the dimmed neighbors do not
        // compete with it for the eye.
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

/// A pipeline differs from its neighbor in four things, and exactly those are
/// the parameters here: what to paint with, what to draw, and how to handle
/// depth. The vertex function, the bind group layout and the target format are
/// shared by them.
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
            // Position and normal as the Vertex fields, offsets are derived.
            attributes: gfx::packed_attributes(&[
                VertexFormat::VtxFloat32x3,
                VertexFormat::VtxFloat32x3,
                VertexFormat::VtxFloat32x3,
            ]),
        }],
        bind_group_layout_ids: vec![camera_layout.id()],
        primitive_topology: topology as i32,
        // No culling: a closed surface looks the same without it, and the depth
        // test solves the task — the grid needs it too, since its faces cannot
        // be told apart.
        cull_mode: CullMode::None as i32,
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT as i32,
            depth_write_enabled: depth_write,
            depth_compare: depth_compare as i32,
        }),
        ..Default::default()
    })
}
