//! Ресурсы устройства и запись кадра.
//!
//! Раздел тот же, что у глобуса: [`Device`] — пайплайн, сэмплер и layout
//! привязки тайла, которым размер места безразличен; [`Target`] — view чужой
//! текстуры, который им и определяется. Буфера глубины нет: кадр плоский,
//! ячейки не перекрываются по построению (каждая рисуется ровно одним
//! тайлом), и порядок задаёт сама очередь команд.

use veldsdk::graphics::{
    self as gfx, buffer_usage, BindGroupId, BindGroupLayoutId, CreateRenderPipeline, CullMode,
    FilterMode, GrowingBuffer, PipelineId, PrimitiveTopology, SamplerId, StepMode,
    TextureViewId, VertexBufferLayout, VertexFormat, VISIBILITY_FRAGMENT,
};

/// Вершина квада тайла: позиция уже в NDC, uv — в текстуре тайла. Пересчёт
/// из пикселей снимка сделан на CPU в f64 (см. camera.rs): в шейдер числа
/// приходят уже относительными к кадру, где f32 хватает.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// Насколько сильно место этой ячейки подкрашено синевой ряби, 0..1.
    ///
    /// Готовой величиной, а не фазой с признаком, как у шара: сетка канвы
    /// переписывается каждым кадром целиком, и волну дешевле испечь на месте,
    /// чем везти в шейдер две величины ради того же самого. У шара сетка живёт
    /// между кадрами — там иначе и нельзя.
    pub glow: f32,
}

/// Квад одного тайла: прямоугольник кадра, кусок текстуры и её привязка.
pub struct Quad {
    /// x0, y0, x1, y1 в NDC.
    pub rect: [f32; 4],
    /// u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub bind: BindGroupId,
    /// Сила ряби на этой ячейке (см. [`Vertex::glow`]).
    pub glow: f32,
}

pub struct Device {
    pipeline: PipelineId,
    tile_layout: BindGroupLayoutId,
    sampler: SamplerId,
    /// Формат таргета, под который собран пайплайн. Сменится — пересобирать.
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
        // Linear в обе стороны: ужатие в кадре не глубже двух крат (уровень
        // выбирается по масштабу), а тайлы sRGB — сэмплер отдаёт линейное,
        // и усреднение соседних текселей честное.
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

    /// Привязка одного тайла: его view и общий сэмплер.
    pub fn tile_bind_group(&self, view: &TextureViewId) -> anyhow::Result<BindGroupId> {
        gfx::create_bind_group(
            "Image Tile BG",
            &self.tile_layout,
            vec![gfx::texture_entry(0, view), gfx::sampler_entry(1, &self.sampler)],
        )
    }
}

/// Место, отданное нам владельцем разметки.
pub struct Target {
    /// Текстура не наша: держит её владелец, здесь только id — по нему видно,
    /// что таргет сменился.
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

/// Буфер вершин канвы. У каждой канвы свой: команды исполняются кадровым
/// циклом хоста позже записи, и один общий буфер на две канвы обе рисовал бы
/// содержимым последней.
pub fn vertex_buffer() -> GrowingBuffer {
    // Экран — десятки тайлов; 16 КиБ — это сотня квадов с запасом.
    GrowingBuffer::new(buffer_usage::VERTEX, "вершины тайлов", 16 * 1024)
}

/// Кадр: квады в буфер и команды в очередь. Пустой набор квадов тоже кадр —
/// он стирает прежнее содержимое (пасс хоста чистит таргет прозрачным).
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Раскладка вершины и её длина считаются в двух разных местах: шаг берётся
    /// из `size_of`, а смещения — из перечня форматов. Разъедься они — пайплайн
    /// соберётся, кадр запишется, а на канве окажется пустое место: имени у неё
    /// нет, обходу разметки она себя не объявляет, и сценарию такой отказ не
    /// виден вовсе.
    #[test]
    fn раскладка_вершины_сходится_с_её_шагом() {
        let attributes = gfx::packed_attributes(&[
            VertexFormat::VtxFloat32x2,
            VertexFormat::VtxFloat32x2,
            VertexFormat::VtxFloat32,
        ]);
        let spanned = attributes
            .last()
            .map_or(0, |last| last.offset as usize + format_size(last.format));
        assert_eq!(spanned, std::mem::size_of::<Vertex>());
    }

    /// Сколько байт занимает атрибут такого формата. Своим перечнем, а не общей
    /// функцией: сойтись он обязан не с кодом, а с тем, что понимает драйвер.
    fn format_size(format: i32) -> usize {
        match format {
            f if f == VertexFormat::VtxFloat32 as i32 => 4,
            f if f == VertexFormat::VtxFloat32x2 as i32 => 8,
            f if f == VertexFormat::VtxFloat32x3 as i32 => 12,
            f if f == VertexFormat::VtxFloat32x4 as i32 => 16,
            other => unreachable!("неизвестный формат вершины: {other}"),
        }
    }
}
