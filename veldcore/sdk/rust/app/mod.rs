pub use crate::rpc::app::*;

crate::rpc_proxy! {
    service: "app",
    display: AppDisplayCommand => (),
}

pub struct AppBridge;
impl AppBridge {
    pub fn display_frame() -> anyhow::Result<()> {
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(DrawFrame {}))
        };
        raw::display(&cmd)
    }
}
