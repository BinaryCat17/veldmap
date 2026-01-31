// use veldmap_render::create_renderer;

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();
    
    println!("Starting VeldMap Desktop Client...");
    println!("Note: Renderer initialization is temporarily disabled during refactoring.");
}