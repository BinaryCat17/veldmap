use super::proto::{
    render_command::Command, CommandBuffer, Draw, DrawIndexed, RenderCommand,
    SetBindGroup, SetIndexBuffer, SetPipeline, SetScissorRect, SetVertexBuffer,
    SetViewport, Submit,
};
use super::{BindGroupId, IndexFormat, PipelineId, TextureViewId};
use prost::Message;

/// Записывает render-команды кадра и отправляет их хосту одним submit'ом.
/// Хост исполняет команды в своём цикле кадра, в указанный texture view.
#[derive(Default)]
pub struct RenderRecorder {
    commands: Vec<RenderCommand>,
}

impl RenderRecorder {
    pub fn new() -> Self {
        Self { commands: Vec::with_capacity(128) }
    }

    fn push(&mut self, cmd: Command) {
        self.commands.push(RenderCommand { command: Some(cmd) });
    }

    pub fn set_pipeline(&mut self, pipeline: PipelineId) {
        self.push(Command::SetPipeline(SetPipeline { pipeline_id: pipeline.0 }));
    }

    pub fn set_bind_group(&mut self, index: u32, bind_group: BindGroupId) {
        self.push(Command::SetBindGroup(SetBindGroup { index, bind_group_id: bind_group.0 }));
    }

    /// `buffer_region` — region id буфера из memory ABI; `size == 0` — до конца буфера.
    pub fn set_vertex_buffer(&mut self, slot: u32, buffer_region: u64, offset: u64, size: u64) {
        self.push(Command::SetVertexBuffer(SetVertexBuffer {
            slot, buffer_id: buffer_region, offset, size,
        }));
    }

    pub fn set_index_buffer(&mut self, buffer_region: u64, format: IndexFormat, offset: u64, size: u64) {
        self.push(Command::SetIndexBuffer(SetIndexBuffer {
            buffer_id: buffer_region, index_format: format as i32, offset, size,
        }));
    }

    pub fn draw(&mut self, vertices: std::ops::Range<u32>, instances: std::ops::Range<u32>) {
        self.push(Command::Draw(Draw {
            vertex_count: vertices.end - vertices.start,
            instance_count: instances.end - instances.start,
            first_vertex: vertices.start,
            first_instance: instances.start,
        }));
    }

    pub fn draw_indexed(&mut self, indices: std::ops::Range<u32>, base_vertex: i32, instances: std::ops::Range<u32>) {
        self.push(Command::DrawIndexed(DrawIndexed {
            index_count: indices.end - indices.start,
            instance_count: instances.end - instances.start,
            first_index: indices.start,
            base_vertex,
            first_instance: instances.start,
        }));
    }

    pub fn set_viewport(&mut self, x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32) {
        self.push(Command::SetViewport(SetViewport { x, y, width, height, min_depth, max_depth }));
    }

    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.push(Command::SetScissorRect(SetScissorRect { x, y, width, height }));
    }

    /// Отдаёт записанные команды хосту; исполнятся на следующем кадре.
    pub fn submit(self, target: TextureViewId) -> anyhow::Result<()> {
        let submit = Submit {
            target_texture_view_id: target.0,
            command_buffer: Some(CommandBuffer { commands: self.commands }),
        };
        crate::abi::graphics_execute(submit.encode_to_vec())?;
        Ok(())
    }
}
