use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::ui::UiDisplayCommand;
use prost::Message;
use tokio::sync::mpsc;
use winit::event_loop::EventLoopProxy;

pub enum AppCommand {
    Draw(Vec<u8>, u32, u32),
}

pub struct AppService {
    tx: mpsc::UnboundedSender<AppCommand>,
    proxy: EventLoopProxy<()>,
    is_visible: Arc<AtomicBool>,
}

impl AppService {
    pub fn new(tx: mpsc::UnboundedSender<AppCommand>, proxy: EventLoopProxy<()>, is_visible: Arc<AtomicBool>) -> Self {
        Self { tx, proxy, is_visible }
    }
}

impl NativeService for AppService {
    fn call(&self, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match method {
            "display" => {
                let cmd = UiDisplayCommand::decode(&payload[..])?;
                match cmd.command {
                    Some(veldmap_host_core::ui::ui_display_command::Command::DrawFrame(frame)) => {
                        let _ = self.tx.send(AppCommand::Draw(frame.rgba_data, frame.width, frame.height));
                        if self.is_visible.load(Ordering::SeqCst) {
                            let _ = self.proxy.send_event(());
                        }
                        Ok(Vec::new())
                    }
                    _ => Err(anyhow::anyhow!("Unsupported display command")),
                }
            }
            _ => Err(anyhow::anyhow!("Unknown app method: {}", method)),
        }
    }
}