pub use crate::rpc::app::*;

crate::host_proxy! {
    service: "app",
    display: AppDisplayCommand => (),
}

pub struct AppBridge;
impl AppBridge {
    /// Submit a frame for display using the given texture ID
    pub fn display_frame(texture_id: u64) -> anyhow::Result<()> {
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(DrawFrame { texture_id }))
        };
        raw::display(&cmd)
    }
}
