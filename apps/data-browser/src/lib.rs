use veldmap_rust_rpc::ui::{DrawFrame, UiDisplayCommand, ui_display_command};
use veldmap_rust_rpc::host::call_service;
use prost::Message as ProstMessage;
use std::sync::Mutex;
use lazy_static::lazy_static;
use std::cell::RefCell;
use extism_pdk::{plugin_fn, FnResult};

mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;

use app::VeldMapToolsGui;
use iced_graphics::Viewport;
use iced_core::{Size, Theme, mouse, Color, Pixels, Rectangle, Point, keyboard};
use iced_tiny_skia::Renderer;
use iced_runtime::user_interface::{self, UserInterface};

lazy_static! {
    static ref GUI: Mutex<Option<VeldMapToolsGui>> = Mutex::new(None);
    static ref CANVAS_SIZE: Mutex<(u32, u32)> = Mutex::new((1024, 768));
    static ref CURSOR_POSITION: Mutex<Point> = Mutex::new(Point::ORIGIN);
    static ref PENDING_EVENTS: Mutex<Vec<iced_core::Event>> = Mutex::new(Vec::new());
}

const SCALE_FACTOR: f32 = 1.5;

thread_local! {
    static INTERFACE_CACHE: RefCell<user_interface::Cache> = RefCell::new(user_interface::Cache::default());
    static RENDERER: RefCell<Renderer> = RefCell::new(Renderer::new(common::APP_FONT, Pixels(16.0)));
    static FONTS_LOADED: RefCell<bool> = RefCell::new(false);
}

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
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
            if let Some(ev) = event_proto.event {
                match ev {
                    veldmap_rust_rpc::ui::ui_event::Event::Resize(r) => { 
                        *CANVAS_SIZE.lock().unwrap() = (r.width, r.height); 
                    }
                    veldmap_rust_rpc::ui::ui_event::Event::Click(c) => {
                        let logical_x = c.x / SCALE_FACTOR;
                        let logical_y = c.y / SCALE_FACTOR;
                        *CURSOR_POSITION.lock().unwrap() = Point::new(logical_x, logical_y);
                        
                        let mut events = PENDING_EVENTS.lock().unwrap();
                        let button = match c.button {
                            1 => mouse::Button::Left,
                            2 => mouse::Button::Right,
                            3 => mouse::Button::Middle,
                            _ => mouse::Button::Left,
                        };
                        events.push(iced_core::Event::Mouse(mouse::Event::CursorMoved { position: Point::new(logical_x, logical_y) }));
                        events.push(iced_core::Event::Mouse(mouse::Event::ButtonPressed(button)));
                        events.push(iced_core::Event::Mouse(mouse::Event::ButtonReleased(button)));
                    }
                    veldmap_rust_rpc::ui::ui_event::Event::Key(k) => {
                        let mut events = PENDING_EVENTS.lock().unwrap();
                        let key = if k.key_code == 13 {
                            keyboard::Key::Named(keyboard::key::Named::Enter)
                        } else if k.key_code == 8 {
                            keyboard::Key::Named(keyboard::key::Named::Backspace)
                        } else {
                            keyboard::Key::Unidentified
                        };

                        if key != keyboard::Key::Unidentified {
                            let physical_key = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
                            if k.pressed {
                                events.push(iced_core::Event::Keyboard(keyboard::Event::KeyPressed {
                                    key: key.clone(),
                                    modifiers: keyboard::Modifiers::default(),
                                    location: keyboard::Location::Standard,
                                    text: None,
                                    modified_key: key,
                                    physical_key,
                                    repeat: false,
                                }));
                            } else {
                                events.push(iced_core::Event::Keyboard(keyboard::Event::KeyReleased {
                                    key: key.clone(),
                                    modifiers: keyboard::Modifiers::default(),
                                    location: keyboard::Location::Standard,
                                    modified_key: key,
                                    physical_key,
                                }));
                            }
                        }
                    }
                    _ => {}
                }
            }
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

    let cursor_pos = *CURSOR_POSITION.lock().unwrap();
    let cursor = mouse::Cursor::Available(cursor_pos);
    
    let events = std::mem::take(&mut *PENDING_EVENTS.lock().unwrap());

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut pixmap = tiny_skia::PixmapMut::from_bytes(&mut pixels, width, height).unwrap();
    pixmap.fill(tiny_skia::Color::from_rgba8(20, 23, 26, 255)); 

    let viewport = Viewport::with_physical_size(Size::new(width, height), SCALE_FACTOR);
    
    RENDERER.with(|renderer_cell| {
        let mut renderer = renderer_cell.borrow_mut();
        
        FONTS_LOADED.with(|loaded_cell| {
            let mut loaded = loaded_cell.borrow_mut();
            if !*loaded {
                let fs = iced_graphics::text::font_system();
                let mut fs_write = fs.write().unwrap();
                let db = fs_write.raw().db_mut();
                db.load_font_data(common::DEJAVU_FONT_DATA.to_vec());
                db.load_font_data(common::EMOJI_FONT_DATA.to_vec());
                *loaded = true;
            }
        });

        INTERFACE_CACHE.with(|cache_cell| {
            let mut cache = std::mem::take(&mut *cache_cell.borrow_mut());
            let mut messages = Vec::new();
            
            {
                let mut ui = UserInterface::build(
                    gui.view(),
                    viewport.logical_size(),
                    cache,
                    &mut *renderer,
                );

                let mut clipboard = iced_core::clipboard::Null;
                ui.update(&events, cursor, &mut *renderer, &mut clipboard, &mut messages);
                cache = ui.into_cache();
            }

            if !messages.is_empty() {
                for message in messages {
                    let _ = gui.update(message);
                }
            }

            let mut user_interface = UserInterface::build(
                gui.view(),
                viewport.logical_size(),
                cache,
                &mut *renderer,
            );

            user_interface.draw(&mut *renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
            
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
