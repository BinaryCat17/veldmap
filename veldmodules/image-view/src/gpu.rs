use veldsdk::graphics::{
    self as gfx, buffer_usage, BindGroupId, BindGroupLayoutId, CreateRenderPipeline, CullMode,
    FilterMode, GrowingBuffer, PipelineId, PrimitiveTopology, SamplerId, StepMode,
    TextureViewId, VertexBufferLayout, VertexFormat, VISIBILITY_FRAGMENT,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub glow: f32,
}

pub struct Quad {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub bind: BindGroupId,
    pub glow: f32,
}

pub struct Device {
    pipeline: PipelineId,
    tile_layout: BindGroupLayoutId,
    sampler: SamplerId,
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

    pub fn tile_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Image Tile BG",
            &self.tile_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.sampler)],
        )
    }
}

pub struct Target {
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

pub fn vertex_buffer() -> GrowingBuffer {
    GrowingBuffer::new(buffer_usage::VERTEX, "tile vertices", 16 * 1024)
}

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
