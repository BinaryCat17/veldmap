//! Device resources and frame recording.
//!
//! The split is the same as in the globe: [`Device`] — pipeline, sampler, and
//! the tile bind group layout, which care nothing about the size of the
//! allocated space; [`Target`] — a view of a foreign texture, which is exactly
//! what defines it. There is no depth buffer: the frame is flat, cells never
//! overlap by construction (each is drawn by exactly one tile), and order comes
//! from the command queue itself.

use veldsdk::graphics::{
    self as gfx, buffer_usage, BindGroupId, BindGroupLayoutId, CreateRenderPipeline, CullMode,
    FilterMode, GrowingBuffer, PipelineId, PrimitiveTopology, SamplerId, StepMode,
    TextureViewId, VertexBufferLayout, VertexFormat, VISIBILITY_FRAGMENT,
};

/// Tile quad vertex: position is already in NDC, uv lives in the tile texture.
/// The conversion from image pixels is done on the CPU in f64 (see camera.rs):
/// numbers reach the shader already relative to the frame, where f32 suffices.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// How strongly this cell's spot is tinted with the loading ripple blue, 0..1.
    ///
    /// A ready-made value, not a phase with a flag as on the globe: the canvas
    /// mesh is rewritten in full every frame, and baking the wave on the spot is
    /// cheaper than carrying two values into the shader for the same result. The
    /// globe mesh lives across frames — there it cannot be otherwise.
    pub glow: f32,
}

/// Quad of a single tile: a frame rectangle, a piece of texture, and its bind group.
pub struct Quad {
    /// x0, y0, x1, y1 in NDC.
    pub rect: [f32; 4],
    /// u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub bind: BindGroupId,
    /// Ripple strength on this cell (see [`Vertex::glow`]).
    pub glow: f32,
}

pub struct Device {
    pipeline: PipelineId,
    tile_layout: BindGroupLayoutId,
    sampler: SamplerId,
    /// Target format the pipeline is built for. A change means a rebuild.
    pub format: i32,
}

impl Device {
    pub fn create(format: i32) -> anyhow::Result<Self> {
        let shader = gfx::create_shader(include_str!("image_view.wgsl"), "Image View Shader")?;
        let tile_layout = gfx::create_bind_group_layout(
            "Image Tile BGL",
            vec![
                gfx::texture_layout_entry(0, VISIBILITY_FRAGMENT),
                gfx::sampler_layout_entry(1, VISIBILITY_FRAGMENT),
            ],
        )?;
        // Linear in both directions: the frame's downsampling is never deeper
        // than twofold (the level is chosen by scale), and the tiles are sRGB —
        // the sampler hands out linear values, so averaging neighboring texels
        // stays honest.
        let sampler = gfx::create_sampler(FilterMode::FiltLinear, FilterMode::FiltLinear)?;

        let pipeline = gfx::create_render_pipeline(CreateRenderPipeline {
            shader_id: shader.id(),
            label: "Image Tiles".into(),
            vertex_entry: "vs_main".into(),
            fragment_entry: "fs_main".into(),
            target_format: format,
            vertex_layouts: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: StepMode::StepVertex as i32,
                attributes: gfx::packed_attributes(&[
                    VertexFormat::VtxFloat32x2,
                    VertexFormat::VtxFloat32x2,
                    VertexFormat::VtxFloat32,
                ]),
            }],
            bind_group_layout_ids: vec![tile_layout.id()],
            primitive_topology: PrimitiveTopology::TopologyTriangleList as i32,
            cull_mode: CullMode::None as i32,
            depth_stencil: None,
            ..Default::default()
        })?;

        Ok(Self { pipeline, tile_layout, sampler, format })
    }

    /// Bind group of a single tile: its view and the shared sampler.
    pub fn tile_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Image Tile BG",
            &self.tile_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.sampler)],
        )
    }
}

/// Space handed to us by the layout owner.
pub struct Target {
    /// The texture is not ours: its owner holds it, only the id lives here — it
    /// shows when the target has changed.
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    view: TextureViewId,
}

impl Target {
    pub fn create(texture_id: u64, width: u32, height: u32) -> anyhow::Result<Self> {
        let view = gfx::create_texture_view(texture_id)?;
        Ok(Self { texture_id, width, height, view })
    }
}

/// Canvas vertex buffer. Each canvas has its own: commands execute in the host's
/// frame loop after the write, and one shared buffer for two canvases would draw
/// both with the last one's content.
pub fn vertex_buffer() -> GrowingBuffer {
    // The screen holds dozens of tiles; 16 KiB fits a hundred quads with room to spare.
    GrowingBuffer::new(buffer_usage::VERTEX, "tile vertices", 16 * 1024)
}

/// A frame: quads into the buffer, commands into the queue. An empty quad set is
/// a frame too — it wipes the previous content (the host pass clears the target
/// to transparent).
pub fn render(
    device: &Device,
    target: &Target,
    vertices: &mut GrowingBuffer,
    quads: &[Quad],
) -> anyhow::Result<()> {
    let mut mesh = Vec::with_capacity(quads.len() * 6);
    for quad in quads {
        let [x0, y0, x1, y1] = quad.rect;
        let [u0, v0, u1, v1] = quad.uv;
        let glow = quad.glow;
        let corners = [
            Vertex { pos: [x0, y0], uv: [u0, v0], glow },
            Vertex { pos: [x1, y0], uv: [u1, v0], glow },
            Vertex { pos: [x0, y1], uv: [u0, v1], glow },
            Vertex { pos: [x1, y0], uv: [u1, v0], glow },
            Vertex { pos: [x1, y1], uv: [u1, v1], glow },
            Vertex { pos: [x0, y1], uv: [u0, v1], glow },
        ];
        mesh.extend_from_slice(&corners);
    }
    vertices.write(&mesh)?;

    let mut recorder = gfx::RenderRecorder::new();
    recorder.set_viewport(0.0, 0.0, target.width as f32, target.height as f32, 0.0, 1.0);
    if let Some(buffer) = vertices.id() {
        recorder.set_pipeline(&device.pipeline);
        recorder.set_vertex_buffer(0, buffer, 0, vertices.filled());
        for (i, quad) in quads.iter().enumerate() {
            recorder.set_bind_group(0, &quad.bind);
            let base = (i * 6) as u32;
            recorder.draw(base..base + 6, 0..1);
        }
    }
    recorder.submit(&target.view)
}
