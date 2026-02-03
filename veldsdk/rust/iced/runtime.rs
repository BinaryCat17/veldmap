#[cfg(feature = "graphics")]
use crate::graphics::UiBridge;
use crate::iced::RawIcedRuntime;
use crate::core::{Command, BoxedFuture};
use crate::rpc::ui::UiEvent;
use prost::Message;
use iced_core::{mouse, keyboard, Size, Theme, Color, Rectangle, Point, Pixels, Font, Event};
use iced_graphics::Viewport;
use iced_runtime::user_interface::{self, UserInterface};
use iced_tiny_skia::Renderer;
use std::cell::RefCell;
use std::task::{Context, Poll};
use futures_util::task::noop_waker_ref;

/// Internal implementation of the Iced runtime that uses closures for flexibility.
pub struct IcedRuntime<S, M> {
    state: RefCell<S>,
    update_fn: fn(&mut S, M) -> Command<M>,
    view_fn: fn(&S) -> iced_core::Element<'_, M, Theme, Renderer>,
    
    renderer: RefCell<Renderer>,
    interface_cache: RefCell<user_interface::Cache>,
    canvas_size: RefCell<(u32, u32)>,
    scale_factor: RefCell<f32>,
    cursor_position: RefCell<Point>,
    pending_events: RefCell<Vec<Event>>,
    fonts_loaded: RefCell<bool>,
    needs_redrawing: RefCell<bool>,
    font_data: Vec<(&'static str, &'static [u8])>,
    
    tasks: RefCell<Vec<BoxedFuture<M>>>,
    pixel_buffer: RefCell<Vec<u8>>,
    texture_handle: RefCell<Option<crate::rpc::services::ResourceHandle>>,
}

unsafe impl<S, M> Send for IcedRuntime<S, M> {}
unsafe impl<S, M> Sync for IcedRuntime<S, M> {}

impl<S: 'static, M: Send + 'static> IcedRuntime<S, M> {
    pub fn new(
        state: S, 
        update_fn: fn(&mut S, M) -> Command<M>,
        view_fn: fn(&S) -> iced_core::Element<'_, M, Theme, Renderer>,
        default_font: Font, 
        font_data: Vec<(&'static str, &'static [u8])>
    ) -> Self {
        let renderer = Renderer::new(default_font, Pixels(16.0));
        
        Self {
            state: RefCell::new(state),
            update_fn,
            view_fn,
            renderer: RefCell::new(renderer),
            interface_cache: RefCell::new(user_interface::Cache::default()),
            canvas_size: RefCell::new((1024, 768)),
            scale_factor: RefCell::new(1.0),
            cursor_position: RefCell::new(Point::ORIGIN),
            pending_events: RefCell::new(Vec::new()),
            fonts_loaded: RefCell::new(false),
            needs_redrawing: RefCell::new(true),
            font_data,
            tasks: RefCell::new(Vec::new()),
            pixel_buffer: RefCell::new(Vec::new()),
            texture_handle: RefCell::new(None),
        }
    }
}

impl<S: 'static, M: Send + 'static> RawIcedRuntime for IcedRuntime<S, M> {
    fn tick(&self) -> anyhow::Result<()> {
        let mut new_messages = Vec::new();
        
        {
            let mut tasks = self.tasks.borrow_mut();
            if tasks.is_empty() { return Ok(()); }

            let waker = noop_waker_ref();
            let mut cx = Context::from_waker(waker);
            
            tasks.retain_mut(|task| {
                match task.as_mut().poll(&mut cx) {
                    Poll::Ready(maybe_msg) => {
                        if let Some(msg) = maybe_msg {
                            new_messages.push(msg);
                        }
                        false
                    },
                    Poll::Pending => true,
                }
            });
        }

        if !new_messages.is_empty() {
            let mut state = self.state.borrow_mut();
            let mut tasks = self.tasks.borrow_mut();
            for msg in new_messages {
                let command = (self.update_fn)(&mut state, msg);
                tasks.extend(command.0);
            }
            *self.needs_redrawing.borrow_mut() = true;
        }
        Ok(())
    }

    fn handle_event(&self, event_proto: UiEvent) -> anyhow::Result<()> {
        if let Some(ev) = event_proto.event {
            match ev {
                crate::rpc::ui::ui_event::Event::Resize(r) => { 
                    *self.canvas_size.borrow_mut() = (r.width, r.height);
                    *self.scale_factor.borrow_mut() = r.scale_factor;
                    *self.needs_redrawing.borrow_mut() = true;
                }
                crate::rpc::ui::ui_event::Event::CursorMoved(c) => {
                    let sf = *self.scale_factor.borrow();
                    let pos = Point::new(c.x / sf, c.y / sf);
                    *self.cursor_position.borrow_mut() = pos;
                    let mut events = self.pending_events.borrow_mut();
                    events.push(Event::Mouse(mouse::Event::CursorMoved { position: pos }));
                }
                crate::rpc::ui::ui_event::Event::Click(c) => {
                    let sf = *self.scale_factor.borrow();
                    let pos = Point::new(c.x / sf, c.y / sf);
                    *self.cursor_position.borrow_mut() = pos;
                    
                    let button = match c.button {
                        1 => mouse::Button::Left,
                        2 => mouse::Button::Right,
                        3 => mouse::Button::Middle,
                        _ => mouse::Button::Left,
                    };

                    let mut events = self.pending_events.borrow_mut();
                    if c.pressed {
                        events.push(Event::Mouse(mouse::Event::ButtonPressed(button)));
                    } else {
                        events.push(Event::Mouse(mouse::Event::ButtonReleased(button)));
                    }
                }
                crate::rpc::ui::ui_event::Event::Scroll(s) => {
                    let mut events = self.pending_events.borrow_mut();
                    events.push(Event::Mouse(mouse::Event::WheelScrolled { 
                        delta: mouse::ScrollDelta::Pixels { x: s.delta_x, y: s.delta_y } 
                    }));
                }
                crate::rpc::ui::ui_event::Event::Key(k) => {
                    let key = if k.key_code == 13 {
                        keyboard::Key::Named(keyboard::key::Named::Enter)
                    } else if k.key_code == 8 {
                        keyboard::Key::Named(keyboard::key::Named::Backspace)
                    } else {
                        keyboard::Key::Unidentified
                    };

                    if key != keyboard::Key::Unidentified {
                        let physical_key = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
                        let mut events = self.pending_events.borrow_mut();
                        if k.pressed {
                            events.push(Event::Keyboard(keyboard::Event::KeyPressed {
                                key: key.clone(),
                                modifiers: keyboard::Modifiers::default(),
                                location: keyboard::Location::Standard,
                                text: None,
                                modified_key: key,
                                physical_key,
                                repeat: false,
                            }));
                        } else {
                            events.push(Event::Keyboard(keyboard::Event::KeyReleased {
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
        Ok(())
    }

    fn render(&self) -> anyhow::Result<()> {
        let (width, height) = *self.canvas_size.borrow();
        if width == 0 || height == 0 { return Ok(()); }

        let sf = *self.scale_factor.borrow();
        let cursor_pos = *self.cursor_position.borrow();
        let cursor = mouse::Cursor::Available(cursor_pos);
        let events = std::mem::take(&mut *self.pending_events.borrow_mut());

        if !*self.fonts_loaded.borrow() {
            let fs = iced_graphics::text::font_system();
            if let Ok(mut fs_write) = fs.write() {
                let db = fs_write.raw().db_mut();
                for (_, data) in &self.font_data {
                    let _ = db.load_font_data(data.to_vec());
                }
                *self.fonts_loaded.borrow_mut() = true;
            }
        }

        let mut captured_messages = Vec::new();
        let ui_cache;
        let viewport = Viewport::with_physical_size(Size::new(width, height), sf);
        let mut should_draw = *self.needs_redrawing.borrow() || !events.is_empty();

        {
            let mut renderer = self.renderer.borrow_mut();
            let cache = std::mem::take(&mut *self.interface_cache.borrow_mut());
            let state = self.state.borrow();
            
            let mut ui = UserInterface::build(
                (self.view_fn)(&state),
                viewport.logical_size(),
                cache,
                &mut *renderer,
            );

            let mut clipboard = iced_core::clipboard::Null;
            let (state, _event_statuses) = ui.update(&events, cursor, &mut *renderer, &mut clipboard, &mut captured_messages);
            
            if matches!(state, user_interface::State::Outdated) || !captured_messages.is_empty() {
                should_draw = true;
            }

            if should_draw {
                let buffer_size = (width * height * 4) as usize;
                let mut pixels = self.pixel_buffer.borrow_mut();
                if pixels.len() != buffer_size {
                    *pixels = vec![0u8; buffer_size];
                }
                
                // Check or create GPU texture
                let mut texture_borrow = self.texture_handle.borrow_mut();
                let needs_new_texture = texture_borrow.as_ref().map_or(true, |h| h.size != buffer_size as u64);
                
                if needs_new_texture {
                    use crate::rpc::services::{GpuResourceRequest, CreateTexture, GpuResourceResponse};
                    let req = GpuResourceRequest {
                        command: Some(crate::rpc::services::gpu_resource_request::Command::CreateTexture(CreateTexture {
                            width, height, format: 37, // Rgba8Unorm
                            usage: 8, // TEXTURE_BINDING
                        }))
                    };
                    if let Ok(res_bytes) = crate::rpc::host::call_service("system", "create_resource", req.encode_to_vec()) {
                        if let Ok(res) = GpuResourceResponse::decode(&res_bytes[..]) {
                            *texture_borrow = res.handle;
                        }
                    }
                }
                
                let mut pixmap = tiny_skia::PixmapMut::from_bytes(pixels.as_mut_slice(), width, height)
                    .ok_or_else(|| anyhow::anyhow!("Failed to create pixmap"))?;
                
                pixmap.fill(tiny_skia::Color::from_rgba8(20, 23, 26, 255)); 

                ui.draw(&mut *renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);

                if let Some(mut mask) = tiny_skia::Mask::new(width, height) {
                    renderer.draw(
                        &mut pixmap,
                        &mut mask,
                        &viewport,
                        &[Rectangle { x: 0.0, y: 0.0, width: width as f32, height: height as f32 }],
                        Color::TRANSPARENT,
                    );
                }
                
                if let Some(handle_gpu) = texture_borrow.clone() {
                    // Теперь это безопасно и чисто
                    crate::rpc::host::gpu_write_resource(handle_gpu.id, 0, pixels.as_slice())?;
                    UiBridge::display_frame(handle_gpu, width, height)?;
                }
            }
            
            ui_cache = ui.into_cache();
        }

        if !captured_messages.is_empty() {
            let mut state_mut = self.state.borrow_mut();
            let mut tasks = self.tasks.borrow_mut();
            for message in captured_messages {
                let command = (self.update_fn)(&mut state_mut, message);
                tasks.extend(command.0);
            }
            *self.needs_redrawing.borrow_mut() = true;
        }

        *self.interface_cache.borrow_mut() = ui_cache;
        *self.needs_redrawing.borrow_mut() = false;

        Ok(())
    }
}
