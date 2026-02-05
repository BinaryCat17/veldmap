use crate::rpc::ui::{RenderCommands, UiDisplayCommand, ui_display_command};
use crate::rpc::wgpu::{
    CommandBuffer, WgpuCommand, SetPipeline, SetBindGroup, SetVertexBuffer, 
    SetIndexBuffer, Draw, DrawIndexed, SetViewport, SetScissorRect,
    wgpu_command::Command
};
use crate::rpc::host::call_service;
use prost::Message;

pub struct WgpuRecorder {
    commands: Vec<WgpuCommand>,
    width: u32,
    height: u32,
}

impl WgpuRecorder {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            commands: Vec::with_capacity(128),
            width,
            height,
        }
    }

    fn push_cmd(&mut self, cmd: Command) {
        self.commands.push(WgpuCommand { command: Some(cmd) });
    }

    pub fn set_pipeline(&mut self, pipeline_id: u64) {
        self.push_cmd(Command::SetPipeline(SetPipeline { pipeline_id }));
    }

    pub fn set_bind_group(&mut self, index: u32, bind_group_id: u64) {
        self.push_cmd(Command::SetBindGroup(SetBindGroup { 
            index, 
            bind_group_id,
            dynamic_offsets: Vec::new()
        }));
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, buffer_id: u64, offset: u64, size: u64) {
        self.push_cmd(Command::SetVertexBuffer(SetVertexBuffer { 
            slot, buffer_id, offset, size 
        }));
    }

    pub fn set_index_buffer(&mut self, buffer_id: u64, format: u32, offset: u64, size: u64) {
        self.push_cmd(Command::SetIndexBuffer(SetIndexBuffer { 
            buffer_id, index_format: format, offset, size 
        }));
    }

    pub fn draw(&mut self, vertices: std::ops::Range<u32>, instances: std::ops::Range<u32>) {
        self.push_cmd(Command::Draw(Draw { 
            vertex_count: vertices.end - vertices.start,
            instance_count: instances.end - instances.start,
            first_vertex: vertices.start,
            first_instance: instances.start
        }));
    }

    pub fn draw_indexed(&mut self, indices: std::ops::Range<u32>, base_vertex: i32, instances: std::ops::Range<u32>) {
        self.push_cmd(Command::DrawIndexed(DrawIndexed { 
            index_count: indices.end - indices.start,
            instance_count: instances.end - instances.start,
            first_index: indices.start,
            base_vertex,
            first_instance: instances.start
        }));
    }

    pub fn set_viewport(&mut self, x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32) {
        self.push_cmd(Command::SetViewport(SetViewport { 
            x, y, width, height, min_depth, max_depth 
        }));
    }

    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.push_cmd(Command::SetScissorRect(SetScissorRect { 
            x, y, width, height 
        }));
    }

    pub fn submit(self) -> anyhow::Result<()> {
        let cmd_buffer = CommandBuffer {
            commands: self.commands,
        };
        
        let render_cmds = RenderCommands {
            width: self.width,
            height: self.height,
            command_buffer: Some(cmd_buffer),
        };
        
        let display_cmd = UiDisplayCommand {
            command: Some(ui_display_command::Command::RenderCommands(render_cmds)),
        };
        
        call_service("app", "display", display_cmd.encode_to_vec())?;
        Ok(())
    }
}