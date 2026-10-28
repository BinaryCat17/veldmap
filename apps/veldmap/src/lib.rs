mod app;
mod platform;

use veldmap_core as _;
use veldmap_render as _;
use veldmap_storage as _;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

pub fn run() {
    app::run();
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    run();
}
