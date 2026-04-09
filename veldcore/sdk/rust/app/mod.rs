pub use crate::rpc::app::*;

crate::host_proxy! {
    service: "app",
    display: AppDisplayCommand => (),
}

pub struct AppBridge;
impl AppBridge {
    pub fn display_frame() -> anyhow::Result<()> {
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(DrawFrame { texture_id: 0 }))
        };
        raw::display(&cmd)
    }
    
    pub fn display_frame_with_id(texture_id: u64) -> anyhow::Result<()> {
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(DrawFrame { texture_id }))
        };
        raw::display(&cmd)
    }
}
