use veldmap_rust_rpc::ui::{DrawFrame, UiDisplayCommand, ui_display_command};
use veldmap_rust_rpc::host::call_service;
use prost::Message as ProstMessage;
use std::sync::Mutex;
use lazy_static::lazy_static;
use std::cell::RefCell;

mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;

use app::VeldMapToolsGui;
use iced_graphics::Viewport;
use iced_core::{Size, Theme, mouse, Color, Pixels, Rectangle};
use iced_tiny_skia::Renderer;
use iced_runtime::user_interface::{self, UserInterface};

lazy_static! {
    static ref GUI: Mutex<Option<VeldMapToolsGui>> = Mutex::new(None);
    static ref CANVAS_SIZE: Mutex<(u32, u32)> = Mutex::new((1024, 768));
}

thread_local! {
    static INTERFACE_CACHE: RefCell<user_interface::Cache> = RefCell::new(user_interface::Cache::default());
    static RENDERER: RefCell<Renderer> = RefCell::new(Renderer::new(common::APP_FONT, Pixels(16.0)));
    static FONTS_LOADED: RefCell<bool> = RefCell::new(false);
}

// Мост для логов
struct HostLogger;
impl log::Log for HostLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool { true }
    fn log(&self, record: &log::Record) {
        let msg = format!("[WASM-LOG] {}: {}", record.level(), record.args());
        let _ = call_service("system", "log", msg.as_bytes().to_vec());
    }
    fn flush(&self) {}
}
static LOGGER: HostLogger = HostLogger;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    // Инициализируем логи один раз
    static LOG_INIT: std::sync::Once = std::sync::Once::new();
    LOG_INIT.call_once(|| {
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    });

    let request = veldmap_rust_rpc::services::RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    match request.method.as_str() {
        "init" => {
            let (gui, _task) = VeldMapToolsGui::new();
            *GUI.lock().unwrap() = Some(gui);
            render_and_send();
        }
        "handle_ui_event" => {
            let event_proto = veldmap_rust_rpc::ui::UiEvent::decode(&request.payload[..]).unwrap();
            let mut gui_lock = GUI.lock().unwrap();
            if let Some(gui) = gui_lock.as_mut() {
                if let Some(ev) = event_proto.event {
                    match ev {
                        veldmap_rust_rpc::ui::ui_event::Event::Resize(r) => { *CANVAS_SIZE.lock().unwrap() = (r.width, r.height); }
                        _ => {}
                    }
                }
            }
            drop(gui_lock);
            render_and_send();
        }
        "render" => { render_and_send(); }
        _ => {}
    };

    let response = veldmap_rust_rpc::services::RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
    Ok(response.encode_to_vec())
}

fn render_and_send() {
    let mut gui_lock = GUI.lock().unwrap();
    let gui = match gui_lock.as_mut() { Some(g) => g, None => return };

    let (width, height) = *CANVAS_SIZE.lock().unwrap();
    if width == 0 || height == 0 { return; }

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut pixmap = tiny_skia::PixmapMut::from_bytes(&mut pixels, width, height).unwrap();
    pixmap.fill(tiny_skia::Color::from_rgba8(40, 44, 52, 255)); 

    let viewport = Viewport::with_physical_size(Size::new(width, height), 1.0);
    
    RENDERER.with(|renderer_cell| {
        let mut renderer = renderer_cell.borrow_mut();
        
        // Загружаем шрифты только один раз для этого потока
        FONTS_LOADED.with(|loaded_cell| {
            let mut loaded = loaded_cell.borrow_mut();
            if !*loaded {
                let fs = iced_graphics::text::font_system();
                let mut fs_write = fs.write().unwrap();
                let db = fs_write.raw().db_mut();
                db.load_font_data(common::DEJAVU_FONT_DATA.to_vec());
                db.load_font_data(common::EMOJI_FONT_DATA.to_vec());
                
                *loaded = true;
                let _ = call_service("system", "log", "WASM: Fonts loaded into global FontSystem".as_bytes().to_vec());
            }
        });

        INTERFACE_CACHE.with(|cache_cell| {
            let cache = std::mem::take(&mut *cache_cell.borrow_mut());

            let mut user_interface = UserInterface::build(
                gui.view(),
                Size::new(width as f32, height as f32),
                cache,
                &mut *renderer,
            );

            let mut messages = Vec::new();
            let mut clipboard = iced_core::clipboard::Null;

            user_interface.update(&[], mouse::Cursor::Unavailable, &mut *renderer, &mut clipboard, &mut messages);
            user_interface.draw(&mut *renderer, &Theme::Dark, &iced_core::renderer::Style::default(), mouse::Cursor::Unavailable);
            
            let mut mask = tiny_skia::Mask::new(width, height).expect("Failed to create mask");

            renderer.draw(
                &mut pixmap,
                &mut mask,
                &viewport,
                &[Rectangle { x: 0.0, y: 0.0, width: width as f32, height: height as f32 }],
                Color::TRANSPARENT,
            );

            *cache_cell.borrow_mut() = user_interface.into_cache();
        });
    });

    let frame = DrawFrame { rgba_data: pixels, width, height };
    let cmd = UiDisplayCommand { command: Some(ui_display_command::Command::DrawFrame(frame)) };
    let _ = call_service("app", "display", cmd.encode_to_vec());
}
