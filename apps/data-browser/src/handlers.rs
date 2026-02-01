use crate::{LocalConfig, LocalState, LocalStateInner, UnsafeSync};
use veldmap_rust_rpc::ui::{UiEvent, DrawFrame, UiDisplayCommand, ui_display_command};
use veldmap_rust_rpc::services::RpcResponse;
use veldmap_rust_rpc::common::Empty;
use veldmap_rust_rpc::host::call_service;
use iced_core::{mouse, keyboard, Size, Theme, Color, Rectangle, Point, Pixels};
use iced_graphics::Viewport;
use iced_runtime::user_interface::UserInterface;
use crate::common;
use crate::app::VeldMapToolsGui;
use prost::Message;
use std::cell::RefCell;
use iced_tiny_skia::Renderer;
use iced_runtime::user_interface;

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    let (gui, _task) = VeldMapToolsGui::new();
    let renderer = Renderer::new(common::APP_FONT, Pixels(16.0));
    
    Ok(LocalState(UnsafeSync(LocalStateInner {
        gui: RefCell::new(gui),
        canvas_size: RefCell::new((1024, 768)),
        scale_factor: RefCell::new(1.0),
        cursor_position: RefCell::new(Point::ORIGIN),
        pending_events: RefCell::new(Vec::new()),
        renderer: RefCell::new(renderer),
        interface_cache: RefCell::new(user_interface::Cache::default()),
        fonts_loaded: RefCell::new(false),
    })))
}

pub(crate) fn handle_ui_event(state: &LocalState, event_proto: UiEvent) -> anyhow::Result<RpcResponse> {
    let inner = &state.0.0;
    if let Some(ev) = event_proto.event {
        match ev {
            veldmap_rust_rpc::ui::ui_event::Event::Resize(r) => { 
                *inner.canvas_size.borrow_mut() = (r.width, r.height); 
                *inner.scale_factor.borrow_mut() = r.scale_factor;
            }
            veldmap_rust_rpc::ui::ui_event::Event::Click(c) => {
                let sf = *inner.scale_factor.borrow();
                let logical_x = c.x / sf;
                let logical_y = c.y / sf;
                *inner.cursor_position.borrow_mut() = Point::new(logical_x, logical_y);
                
                let mut events = inner.pending_events.borrow_mut();
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
                let mut events = inner.pending_events.borrow_mut();
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
    
    render_and_send(state);
    Ok(RpcResponse::default())
}

pub(crate) fn handle_render(state: &LocalState, _req: Empty) -> anyhow::Result<RpcResponse> {
    render_and_send(state);
    Ok(RpcResponse::default())
}

fn render_and_send(state: &LocalState) {
    let inner = &state.0.0;
    let mut gui = inner.gui.borrow_mut();
    let (width, height) = *inner.canvas_size.borrow();
    if width == 0 || height == 0 { return; }

    let cursor_pos = *inner.cursor_position.borrow();
    let cursor = mouse::Cursor::Available(cursor_pos);
    
    let events = std::mem::take(&mut *inner.pending_events.borrow_mut());

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut pixmap = match tiny_skia::PixmapMut::from_bytes(&mut pixels, width, height) {
        Some(p) => p,
        None => return,
    };
    pixmap.fill(tiny_skia::Color::from_rgba8(20, 23, 26, 255)); 

    let sf = *inner.scale_factor.borrow();
    let viewport = Viewport::with_physical_size(Size::new(width, height), sf);
    
    let mut renderer = inner.renderer.borrow_mut();
    let mut cache = std::mem::take(&mut *inner.interface_cache.borrow_mut());

    // Загрузка шрифтов если нужно
    if !*inner.fonts_loaded.borrow() {
        let fs = iced_graphics::text::font_system();
        if let Ok(mut fs_write) = fs.write() {
            let db = fs_write.raw().db_mut();
            let _ = db.load_font_data(common::DEJAVU_FONT_DATA.to_vec());
            let _ = db.load_font_data(common::EMOJI_FONT_DATA.to_vec());
            *inner.fonts_loaded.borrow_mut() = true;
        }
    }

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
    
    if let Ok(mut mask) = tiny_skia::Mask::new(width, height).ok_or("Mask error") {
        renderer.draw(
            &mut pixmap,
            &mut mask,
            &viewport,
            &[Rectangle { x: 0.0, y: 0.0, width: width as f32, height: height as f32 }],
            Color::TRANSPARENT,
        );
    }

    *inner.interface_cache.borrow_mut() = user_interface.into_cache();

    let frame = DrawFrame { rgba_data: pixels, width, height };
    let cmd = UiDisplayCommand { command: Some(ui_display_command::Command::DrawFrame(frame)) };
    let _ = call_service("app", "display", cmd.encode_to_vec());
}