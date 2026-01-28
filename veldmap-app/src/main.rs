#![deny(warnings)]
use winit::{
    event::*,
    event_loop::EventLoop,
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode},
};
use std::sync::Arc;
use veldmap_render::{create_renderer, RenderConfig, RenderBackend};
use veldmap_data::{create_data_provider, Config};
use veldmap_server::create_server;
use veldmap_core::server_module::ServerConfig;

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new()
        .with_title("VeldMap - 3D Earth")
        .build(&event_loop)
        .unwrap());

    // Используем фабрику для создания рендерера с указанием Vulkan
    let veldmap = pollster::block_on(create_renderer(window.clone(), RenderConfig {
        backend: RenderBackend::Vulkan,
    }));

    
    // 1. Запускаем сервер данных (Master) в отдельном системном потоке
    std::thread::spawn(move || {
        let config = ServerConfig {
            addr: "127.0.0.1:3000".parse().unwrap(),
            data_path: std::env::current_dir().unwrap().join("data"),
        };
        let server = create_server(config);
        println!("Server starting on 127.0.0.1:3000...");
        if let Err(e) = server.run() {
            eprintln!("Server error: {}", e);
        }
    });

    // 2. Создаем клиентский провайдер данных
    let data_provider = create_data_provider(Config {
        server_url: "http://127.0.0.1:3000".to_string(),
        cache_path: Some(std::env::current_dir().unwrap().join("cache")),
        use_cache: true,
    });

    // 3. Загружаем геоид в фоне, чтобы не блокировать окно
    let geoid_provider = data_provider.clone();
    let geoid_veldmap = veldmap.clone();
    std::thread::spawn(move || {
        // Даем серверу немного времени на старт
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("Loading geoid from server...");
        if let Ok(geoid) = geoid_provider.get_geoid() {
            println!("Geoid loaded successfully!");
            geoid_veldmap.set_geoid(geoid);
        } else {
            eprintln!("Failed to load geoid");
        }
    });

    let mut last_cursor_pos: Option<(f64, f64)> = None;
    let mut is_left_clicked = false;

    println!("Starting event loop...");
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