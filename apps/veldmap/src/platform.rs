#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

pub fn run() {
    #[cfg(not(target_arch = "wasm32"))]
    native::run();
    #[cfg(target_arch = "wasm32")]
    web::run();
}
