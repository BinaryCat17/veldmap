use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::app::AppDisplayCommand;
use veldmap_host_core::resources::ResourceManager;
use prost::Message;
use tokio::sync::mpsc;
use winit::event_loop::EventLoopProxy;

pub enum AppCommand {
    Draw(u64), // resource_id
}

pub struct AppService {
    tx: mpsc::UnboundedSender<AppCommand>,
    proxy: EventLoopProxy<()>,
    is_visible: Arc<AtomicBool>,
    last_render_time: Arc<Mutex<std::time::Instant>>,
    frame_wake: Arc<tokio::sync::Notify>,
}

impl AppService {
    pub fn new(
        tx: mpsc::UnboundedSender<AppCommand>, 
        proxy: EventLoopProxy<()>, 
        is_visible: Arc<AtomicBool>, 
        _resources: Arc<ResourceManager>,
        last_render_time: Arc<Mutex<std::time::Instant>>,
        frame_wake: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self { tx, proxy, is_visible, last_render_time, frame_wake }
    }
}

impl NativeService for AppService {
    fn call(&self, method: &str, payload: Vec<u8>, _requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "display" => {
                let cmd = AppDisplayCommand::decode(&payload[..])?;
                match cmd.command {
                    Some(veldmap_host_core::app::app_display_command::Command::DrawFrame(_)) => {
                        // Обновляем время отрисовки СРАЗУ, чтобы цикл Frame не уходил в idle
                        if let Ok(mut last) = self.last_render_time.lock() {
                            *last = std::time::Instant::now();
                        }

                        // Любая отрисовка должна будить цикл из спячки
                        self.frame_wake.notify_one();

                        let _ = self.tx.send(AppCommand::Draw(veldmap_host_core::SURFACE_ID));
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