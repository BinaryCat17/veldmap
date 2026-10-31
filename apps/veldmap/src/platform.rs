#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub fn run() -> Result<(), eframe::Error> {
    native::run()
}

#[cfg(target_arch = "wasm32")]
pub fn run() {
    web::run();
}
