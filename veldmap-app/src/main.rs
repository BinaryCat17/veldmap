#![deny(warnings)]
use winit::{
    event::*,
    event_loop::EventLoop,
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode},
};
use std::sync::Arc;
use veldmap_render::create_renderer;
use veldmap_data::{create_data_provider, Config};

#[tokio::main]
async fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new()
        .with_title("VeldMap - 3D Earth")
        .build(&event_loop)
        .unwrap());

    // Используем фабрику для создания рендерера
    let veldmap = create_renderer(window.clone()).await;
    
    // Используем фабрику для создания провайдера данных
    let data_provider = create_data_provider(Config {
        base_path: std::env::current_dir().unwrap().join("data"),
        use_cache: true,
        offline_only: true,
    });

    if let Ok(geoid) = data_provider.get_geoid() {
        veldmap.set_geoid(geoid);
    }

    let mut last_cursor_pos: Option<(f64, f64)> = None;
    let mut is_left_clicked = false;

    let result = event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { ref event, window_id } if window_id == window.id() => {
                match event {
                    WindowEvent::CloseRequested => elwt.exit(),
                    WindowEvent::KeyboardInput {
                        event: KeyEvent {
                            state: ElementState::Pressed,
                            physical_key: PhysicalKey::Code(KeyCode::Escape),
                            ..
                        },
                        ..
                    } => elwt.exit(),
                    WindowEvent::Resized(physical_size) => {
                        veldmap.resize(physical_size.width, physical_size.height);
                    }
                    WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                        is_left_clicked = *state == ElementState::Pressed;
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let scroll = match delta {
                            MouseScrollDelta::LineDelta(_, y) => *y as f64,
                            MouseScrollDelta::PixelDelta(pos) => pos.y / 100.0,
                        };
                        veldmap.camera_zoom(scroll);
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        if is_left_clicked {
                            if let Some((last_x, last_y)) = last_cursor_pos {
                                let dx = position.x - last_x;
                                let dy = position.y - last_y;
                                veldmap.camera_move(dx, dy);
                            }
                        }
                        last_cursor_pos = Some((position.x, position.y));
                    }
                    WindowEvent::RedrawRequested => {
                        veldmap.update();
                        
                        if let Err(e) = veldmap.render() {
                            eprintln!("Render error: {}", e);
                        }
                    }
                    _ => {}
                }
            }
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => {}
        }
    });

    if let Err(e) = result {
        eprintln!("Event loop finished with error: {:?}", e);
    }
}
