#![deny(warnings)]
use winit::{
    event::*,
    event_loop::EventLoop,
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode},
};
use std::sync::Arc;

mod engine;
mod camera;
mod geo;
mod mesh;
mod dem;
#[cfg(test)]
mod engine_tests;

use engine::State;

#[tokio::main]
async fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new()
        .with_title("VeldMap - 3D Earth")
        .build(&event_loop)
        .unwrap());

    let mut state = State::new(window.clone()).await;
    let mut last_cursor_pos: Option<(f64, f64)> = None;

    let result = event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent {
                ref event,
                window_id,
            } if window_id == window.id() => {
                if !state.input(event) {
                    match event {
                        WindowEvent::CloseRequested => {
                            println!("Close requested by OS");
                            elwt.exit();
                        }
                        WindowEvent::KeyboardInput {
                            event:
                                KeyEvent {
                                    state: ElementState::Pressed,
                                    physical_key: PhysicalKey::Code(KeyCode::Escape),
                                    ..
                                },
                            ..
                        } => {
                            println!("Escape pressed");
                            elwt.exit();
                        }
                        WindowEvent::Resized(physical_size) => {
                            state.resize(*physical_size);
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            if let Some((last_x, last_y)) = last_cursor_pos {
                                let dx = position.x - last_x;
                                let dy = position.y - last_y;
                                state.camera_controller.process_mouse_motion(dx, dy, &mut state.camera);
                            }
                            last_cursor_pos = Some((position.x, position.y));
                        }
                        WindowEvent::RedrawRequested => {
                            state.update();
                            match state.render() {
                                Ok(_) => {}
                                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                    eprintln!("Surface lost or outdated, resizing...");
                                    state.resize(state.size);
                                }
                                Err(wgpu::SurfaceError::OutOfMemory) => {
                                    eprintln!("CRITICAL ERROR: GPU Out of Memory!");
                                    elwt.exit();
                                }
                                Err(e) => eprintln!("Render error: {:?}", e),
                            }
                        }
                        _ => {}
                    }
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