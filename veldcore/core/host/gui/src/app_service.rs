use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::app::{AppDisplayCommand, AppDisplayResponse};
use veldmap_host_core::resources::ResourceManager;
use prost::Message;
use tokio::sync::mpsc;
use winit::event_loop::EventLoopProxy;

pub enum AppCommand {
    Draw(u64, u32, u32), // resource_id, width, height
}

pub struct AppService {
    tx: mpsc::UnboundedSender<AppCommand>,
    proxy: EventLoopProxy<()>,
    is_visible: Arc<AtomicBool>,
    monitor_fps: u32,
    actual_fps: Arc<Mutex<f32>>,
    last_render_time: Arc<Mutex<std::time::Instant>>,
    frame_wake: Arc<tokio::sync::Notify>,
}

impl AppService {
    pub fn new(
        tx: mpsc::UnboundedSender<AppCommand>, 
        proxy: EventLoopProxy<()>, 
        is_visible: Arc<AtomicBool>, 
        _resources: Arc<ResourceManager>,
        monitor_fps: u32,
        actual_fps: Arc<Mutex<f32>>,
        last_render_time: Arc<Mutex<std::time::Instant>>,
        frame_wake: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self { tx, proxy, is_visible, monitor_fps, actual_fps, last_render_time, frame_wake }
    }
}

impl NativeService for AppService {
    fn call(&self, method: &str, payload: Vec<u8>, _requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "display" => {
                let cmd = AppDisplayCommand::decode(&payload[..])?;
                match cmd.command {
                    Some(veldmap_host_core::app::app_display_command::Command::DrawFrame(frame)) => {
                        let handle = frame.handle.ok_or_else(|| anyhow::anyhow!("Missing resource handle"))?;
                        
                        // Обновляем время отрисовки СРАЗУ, чтобы цикл Frame не уходил в idle
                        if let Ok(mut last) = self.last_render_time.lock() {
                            *last = std::time::Instant::now();
                        }

                        if frame.request_next_frame {
                            self.frame_wake.notify_one();
                        }

                        let _ = self.tx.send(AppCommand::Draw(handle.id, frame.width, frame.height));
                        if self.is_visible.load(Ordering::SeqCst) {
                            let _ = self.proxy.send_event(());
                        }

                        let response = AppDisplayResponse {
                            monitor_fps: self.monitor_fps,
                            actual_fps: *self.actual_fps.lock().unwrap(),
                        };
                        Ok(response.encode_to_vec())
                    }
                    _ => Err(anyhow::anyhow!("Unsupported display command")),
                }
            }
            _ => Err(anyhow::anyhow!("Unknown app method: {}", method)),
        }
    }
}