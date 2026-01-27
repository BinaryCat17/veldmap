use veldmap::engine::State;
use winit::window::WindowBuilder;
use std::sync::Arc;
use winit::event_loop::EventLoopBuilder;

#[cfg(target_os = "linux")]
use winit::platform::x11::EventLoopBuilderExtX11;

#[tokio::test]
async fn test_engine_state_initialization() {
    let mut builder = EventLoopBuilder::new();
    #[cfg(target_os = "linux")]
    builder.with_any_thread(true);
    
    let event_loop = builder.build().ok();
    
    if let Some(event_loop) = event_loop {
        let window = Arc::new(WindowBuilder::new()
            .with_visible(false)
            .build(&event_loop)
            .unwrap());

        let state = State::new(window).await;
        
        assert!(state.size.width > 0);
        assert!(state.size.height > 0);
        assert!(state.camera.distance > 0.0);
        
        println!("Engine state successfully initialized with GPU adapter.");
    } else {
        println!("Skipping engine test: No display found.");
    }
}