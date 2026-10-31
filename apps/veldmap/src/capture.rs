#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;

#[cfg(not(target_arch = "wasm32"))]
use crate::smoke::UiSnapshot;

#[derive(Clone, Debug, Default)]
pub struct CaptureView {
    pub samples: u64,
    pub colors: u32,
    pub red: u64,
    pub green: u64,
    pub blue: u64,
    pub yellow: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureDiagnostics {
    pub completed: u64,
    pub error: Option<String>,
    pub viewer: CaptureView,
    pub globe: CaptureView,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn request(
    context: &egui::Context,
    diagnostics: &Arc<Mutex<CaptureDiagnostics>>,
    ui: UiSnapshot,
) {
    let diagnostics = Arc::clone(diagnostics);
    let callback_context = context.clone();
    let pixels_per_point = context.pixels_per_point();
    context.request_screenshot(move |image| {
        let viewer = inspect(&image, ui.viewports[0].rect, pixels_per_point);
        let globe = inspect(&image, ui.viewports[1].rect, pixels_per_point);
        let result = save_ppm(&image).map(|()| (viewer, globe));
        complete(&callback_context, &diagnostics, result);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn complete(
    context: &egui::Context,
    diagnostics: &Arc<Mutex<CaptureDiagnostics>>,
    result: Result<(CaptureView, CaptureView), String>,
) {
    let mut diagnostics = diagnostics.lock().expect("capture diagnostics lock");
    diagnostics.completed += 1;
    match result {
        Ok((viewer, globe)) => {
            diagnostics.viewer = viewer;
            diagnostics.globe = globe;
        }
        Err(error) => diagnostics.error = Some(error),
    }
    drop(diagnostics);
    context.request_repaint();
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect(image: &egui::ColorImage, rect: [f32; 4], pixels_per_point: f32) -> CaptureView {
    let [width, height] = image.size;
    let left = ((rect[0] * pixels_per_point) as i32 + 8).clamp(0, width as i32) as usize;
    let top = ((rect[1] * pixels_per_point) as i32 + 8).clamp(0, height as i32) as usize;
    let right = ((rect[2] * pixels_per_point) as i32 - 8).clamp(0, width as i32) as usize;
    let bottom = ((rect[3] * pixels_per_point) as i32 - 8).clamp(0, height as i32) as usize;
    let mut result = CaptureView::default();
    let mut colors = HashSet::new();
    for y in top..bottom {
        for x in left..right {
            let [red, green, blue, _] = image.pixels[y * width + x].to_array();
            result.samples += 1;
            colors.insert(u32::from_be_bytes([red, green, blue, 255]));
            result.red += u64::from(red > 180 && green < 120 && blue < 120);
            result.green += u64::from(red < 120 && green > 120 && blue < 150);
            result.blue += u64::from(red < 120 && green < 180 && blue > 150);
            result.yellow += u64::from(red > 180 && green > 140 && blue < 100);
        }
    }
    result.colors = colors.len() as u32;
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn save_ppm(image: &egui::ColorImage) -> Result<(), String> {
    use std::io::Write as _;

    std::fs::create_dir_all("runtime/logs").map_err(|error| error.to_string())?;
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create("runtime/logs/native-smoke.ppm")
            .map_err(|error| error.to_string())?,
    );
    write!(writer, "P6\n{} {}\n255\n", image.size[0], image.size[1])
        .map_err(|error| error.to_string())?;
    for pixel in &image.pixels {
        writer
            .write_all(&pixel.to_array()[..3])
            .map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())
}
