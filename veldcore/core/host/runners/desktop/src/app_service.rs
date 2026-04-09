use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::app::AppDisplayCommand;
use veldmap_host_core::resources::ResourceManager;
use prost::Message;
use tokio::sync::mpsc;
use winit::event_loop::EventLoopProxy;

#[allow(dead_code)]
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
                    Some(veldmap_host_core::app::app_display_command::Command::DrawFrame(draw_frame)) => {
                        let texture_id = draw_frame.texture_id;
                        veldmap_host_core::vinfo!("AppService::display DrawFrame(texture_id={}), is_visible={}", 
                            texture_id, self.is_visible.load(Ordering::SeqCst));
                        
                        // Обновляем время отрисовки СРАЗУ, чтобы цикл Frame не уходил в idle
                        if let Ok(mut last) = self.last_render_time.lock() {
                            *last = std::time::Instant::now();
                        }

                        // Любая отрисовка должна будить цикл из спячки
                        self.frame_wake.notify_one();

                        // Если texture_id == 0 - используем SURFACE_ID (UI не готов)
                        // Иначе используем texture_id (UI отрисовал в offscreen текстуру)
                        let target_id = if texture_id == 0 { 
                            veldmap_host_core::SURFACE_ID 
                        } else { 
                            texture_id 
                        };
                        
                        let send_result = self.tx.send(AppCommand::Draw(target_id));
                        veldmap_host_core::vinfo!("AppService::display tx.send({}) result: {:?}", target_id, send_result);
                        
                        if self.is_visible.load(Ordering::SeqCst) {
                            let proxy_result = self.proxy.send_event(());
                            veldmap_host_core::vinfo!("AppService::display proxy.send result: {:?}", proxy_result);
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