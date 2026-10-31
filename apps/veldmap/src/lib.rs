mod app;
mod capture;
mod platform;
mod smoke;

use veldmap_core as _;
use veldmap_storage as _;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

pub fn run() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(error) = platform::run() {
        eprintln!("failed to run VeldMap: {error}");
        std::process::exit(1);
    }
    #[cfg(target_arch = "wasm32")]
    platform::run();
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    run();
}
