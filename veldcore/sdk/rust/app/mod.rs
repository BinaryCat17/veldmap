pub use crate::rpc::app::*;

crate::rpc_proxy! {
    service: "app",
    display: AppDisplayCommand => AppDisplayResponse,
}

pub struct AppBridge;
impl AppBridge {
    pub fn display_frame(handle: crate::rpc::core::ResourceHandle, width: u32, height: u32, request_next_frame: bool) -> anyhow::Result<AppDisplayResponse> {
        let cmd = AppDisplayCommand {
            command: Some(app_display_command::Command::DrawFrame(DrawFrame {
                handle: Some(handle),
                width,
                height,
                request_next_frame,
            }))
        };
        raw::display(&cmd)
    }
}
