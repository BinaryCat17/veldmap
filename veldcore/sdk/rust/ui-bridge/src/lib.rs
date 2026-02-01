use veldmap_rust_rpc::ui::{DrawFrame, UiDisplayCommand, ui_display_command};
use veldmap_rust_rpc::host::call_service;
use prost::Message;

pub struct UiBridge;

impl UiBridge {
    pub fn display_frame(rgba_data: Vec<u8>, width: u32, height: u32) -> anyhow::Result<()> {
        let frame = DrawFrame {
            rgba_data,
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
