#[cfg(feature = "graphics")]
use crate::graphics::UiBridge;
use crate::iced::{Application, UiRuntime};
use iced_core::{mouse, Size, Theme, Color, Rectangle, Point, Pixels, Font, Event};
use iced_graphics::Viewport;
use iced_runtime::user_interface::{self, UserInterface};
use iced_tiny_skia::Renderer;
use std::cell::RefCell;

/// Internal implementation of the Iced runtime.
pub struct IcedRuntime<T: Application> {
    gui: RefCell<T>,
    renderer: RefCell<Renderer>,
    interface_cache: RefCell<user_interface::Cache>,
    canvas_size: RefCell<(u32, u32)>,
    scale_factor: RefCell<f32>,
    cursor_position: RefCell<Point>,
    pending_events: RefCell<Vec<Event>>,
    fonts_loaded: RefCell<bool>,
    needs_redrawing: RefCell<bool>,
    font_data: Vec<(&'static str, &'static [u8])>,
}

// We are in WASM, which is single-threaded for now. 
// IcedRuntime uses RefCells, so it's not thread-safe in a generic context,
// but for our plugin architecture, we can safely treat it as such.
unsafe impl<T: Application> Send for IcedRuntime<T> {}
unsafe impl<T: Application> Sync for IcedRuntime<T> {}

impl<T: Application> IcedRuntime<T> {
    pub fn new(gui: T, default_font: Font, font_data: Vec<(&'static str, &'static [u8])>) -> Self {
        let renderer = Renderer::new(default_font, Pixels(16.0));
        
        Self {
            gui: RefCell::new(gui),
            renderer: RefCell::new(renderer),
            interface_cache: RefCell::new(user_interface::Cache::default()),
            canvas_size: RefCell::new((1024, 768)),
            scale_factor: RefCell::new(1.0),
            cursor_position: RefCell::new(Point::ORIGIN),
            pending_events: RefCell::new(Vec::new()),
            fonts_loaded: RefCell::new(false),
            needs_redrawing: RefCell::new(true),
            font_data,
        }
    }
}

impl<T: Application + 'static> UiRuntime for IcedRuntime<T> {
    fn update_size(&self, width: u32, height: u32, scale_factor: f32) {
        *self.canvas_size.borrow_mut() = (width, height);
        *self.scale_factor.borrow_mut() = scale_factor;
        *self.needs_redrawing.borrow_mut() = true;
    }

    fn update_cursor(&self, x: f32, y: f32) {
        let sf = *self.scale_factor.borrow();
        *self.cursor_position.borrow_mut() = Point::new(x / sf, y / sf);
        *self.needs_redrawing.borrow_mut() = true;
    }

    fn cursor_position(&self) -> Point {
        *self.cursor_position.borrow()
    }

    fn push_event(&self, event: Event) {
        self.pending_events.borrow_mut().push(event);
        *self.needs_redrawing.borrow_mut() = true;
    }

    fn render(&self) -> anyhow::Result<()> {
        if !*self.needs_redrawing.borrow() {
            return Ok(());
        }

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

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let mut pixmap = tiny_skia::PixmapMut::from_bytes(&mut pixels, width, height)
            .ok_or_else(|| anyhow::anyhow!("Failed to create pixmap"))?;
        
        pixmap.fill(tiny_skia::Color::from_rgba8(20, 23, 26, 255)); 

        let viewport = Viewport::with_physical_size(Size::new(width, height), sf);
        let mut renderer = self.renderer.borrow_mut();
        let mut cache = std::mem::take(&mut *self.interface_cache.borrow_mut());

        let mut messages = Vec::new();
        {
            let gui = self.gui.borrow();
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
            let mut gui = self.gui.borrow_mut();
            for message in messages {
                gui.update(message);
            }
        }

        let cache = {
            let gui = self.gui.borrow();
            let mut user_interface = UserInterface::build(
                gui.view(),
                viewport.logical_size(),
                cache,
                &mut *renderer,
            );

            user_interface.draw(&mut *renderer, &Theme::Dark, &iced_core::renderer::Style::default(), cursor);
            user_interface.into_cache()
        };
        
        if let Some(mut mask) = tiny_skia::Mask::new(width, height) {
            renderer.draw(
                &mut pixmap,
                &mut mask,
                &viewport,
                &[Rectangle { x: 0.0, y: 0.0, width: width as f32, height: height as f32 }],
                Color::TRANSPARENT,
            );
        }

        *self.interface_cache.borrow_mut() = cache;
        *self.needs_redrawing.borrow_mut() = false;

        UiBridge::display_frame(pixels, width, height)?;
        Ok(())
    }
}