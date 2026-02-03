use crate::rpc::ui::{DrawFrame, UiDisplayCommand, ui_display_command};
use crate::rpc::services::ResourceHandle;
use crate::rpc::host::call_service;
use prost::Message;

pub struct UiBridge;

impl UiBridge {
    pub fn display_frame(handle: ResourceHandle, width: u32, height: u32) -> anyhow::Result<()> {
        let frame = DrawFrame {
            handle: Some(handle),
            width,
            height,
        };
        
        let cmd = UiDisplayCommand {
            command: Some(ui_display_command::Command::DrawFrame(frame)),
        };
        
        call_service("app", "display", cmd.encode_to_vec())?;
        Ok(())
    }
}