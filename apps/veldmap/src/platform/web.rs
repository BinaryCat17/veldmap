use std::cell::RefCell;

use wasm_bindgen::{JsCast as _, JsValue, prelude::wasm_bindgen};

use crate::app::VeldmapApp;

thread_local! {
    static RUNNER: RefCell<Option<eframe::WebRunner>> = const { RefCell::new(None) };
}

pub fn run() {
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("veldmap-canvas"))
        .and_then(|canvas| canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsValue::from_str("missing #veldmap-canvas"))
        .expect("browser canvas");
    let mut options = eframe::WebOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = eframe::egui_wgpu::wgpu::Backends::BROWSER_WEBGPU;
        setup.power_preference = eframe::egui_wgpu::wgpu::PowerPreference::HighPerformance;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let runner = eframe::WebRunner::new();
        let result = runner
            .start(
                canvas,
                options,
                Box::new(|creation| Ok(Box::new(VeldmapApp::new(creation)?))),
            )
            .await;
        match result {
            Ok(()) => RUNNER.with(|slot| *slot.borrow_mut() = Some(runner)),
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

#[wasm_bindgen]
pub fn shutdown() {
    RUNNER.with(|slot| {
        if let Some(runner) = slot.borrow().as_ref() {
            runner.destroy();
        }
    });
    web_sys::console::log_1(&JsValue::from_str("VELDMAP_SHUTDOWN"));
}
