use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::app::AppDisplayCommand;
use prost::Message;
use winit::event_loop::EventLoopProxy;

#[allow(dead_code)]
pub enum AppCommand {
    Draw(u64), // resource_id
}

pub struct AppService {
    proxy: EventLoopProxy<AppCommand>,
}

impl AppService {
    pub fn new(proxy: EventLoopProxy<AppCommand>) -> Self {
        Self { proxy }
    }
}

impl NativeService for AppService {
    fn call(&self, method: &str, payload: Vec<u8>, _requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "display" => {
                let cmd = AppDisplayCommand::decode(&payload[..])?;
                match cmd.command {
                    Some(veldmap_host_core::app::app_display_command::Command::DrawFrame(draw_frame)) => {
                        let texture_id = draw_frame.texture_id;
                        let target_id = if texture_id == 0 { veldmap_host_core::SURFACE_ID } else { texture_id };
                        
                        let _ = self.proxy.send_event(AppCommand::Draw(target_id));
                        
                        Ok(Vec::new())
                    }
                    _ => Err(anyhow::anyhow!("Unsupported display command")),
                }
            }
            _ => Err(anyhow::anyhow!("Unknown app method: {}", method)),
        }
    }
}

pub fn register_services(ctx: std::sync::Arc<veldmap_host_core::setup::HostContext>, proxy: EventLoopProxy<AppCommand>) {
    let app_service = std::sync::Arc::new(AppService::new(proxy));
    ctx.dispatcher.register_service("app".to_string(), veldmap_host_core::dispatcher::ServiceLocation::Native(app_service));
}