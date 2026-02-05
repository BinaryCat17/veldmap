use crate::rpc::app::{DrawFrame, AppDisplayCommand, app_display_command};
use crate::rpc::core::ResourceHandle;
use crate::rpc::host::call_service;
use prost::Message;

pub struct AppBridge;

impl AppBridge {
    pub fn display_frame(handle: ResourceHandle, width: u32, height: u32) -> anyhow::Result<()> {
        let frame = DrawFrame {
            handle: Some(handle),
            width,
            height,
        };
        
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(frame)),
        };
        
        call_service("app", "display", cmd.encode_to_vec())?;
        Ok(())
    }
}
