use std::sync::{Arc, Mutex};

use eframe::egui;
use veldmap_render::{MetricsSnapshot, TILE_ID, ViewportSnapshot};

use crate::capture::{CaptureDiagnostics, CaptureView};

#[derive(Clone, Copy, Debug, Default)]
pub struct ControlPoints {
    pub interact: [f32; 2],
    pub clip: [f32; 2],
    pub scale: [f32; 2],
    pub capture: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewportUi {
    pub rect: [f32; 4],
    pub input: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UiSnapshot {
    pub interactions: u64,
    pub drag_counts: [u64; 2],
    pub clip_enabled: bool,
    pub scale_changes: u64,
    pub controls: ControlPoints,
    pub viewports: [ViewportUi; 2],
}

pub struct SmokeReporter {
    enabled: bool,
    sequence: u64,
    adapter: String,
    backend: String,
    software_gpu: bool,
    capture: Arc<Mutex<CaptureDiagnostics>>,
}

impl SmokeReporter {
    pub fn new(adapter: String, backend: String, software_gpu: bool) -> Self {
        Self {
            enabled: enabled(),
            sequence: 0,
            adapter,
            backend,
            software_gpu,
            capture: Arc::new(Mutex::new(CaptureDiagnostics::default())),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&self) -> Arc<Mutex<CaptureDiagnostics>> {
        Arc::clone(&self.capture)
    }

    pub fn report(
        &mut self,
        context: &egui::Context,
        metrics: &MetricsSnapshot,
        ui: &UiSnapshot,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        self.sequence += 1;
        let viewer = &metrics.viewports[0];
        let globe = &metrics.viewports[1];
        let ready = viewer.draws > 0 && globe.draws > 0;
        let capture = self
            .capture
            .lock()
            .expect("capture diagnostics lock")
            .clone();
        let capture_error = capture.error.as_deref().map(json_string);
        let pointer = context
            .input(|input| input.pointer.latest_pos())
            .map(|position| point([position.x, position.y]))
            .unwrap_or_else(|| "null".to_owned());
        let pointer_down = context.input(|input| input.pointer.primary_down());
        let json = format!(
            "VELDMAP_SMOKE {{\"sequence\":{},\"ready\":{},\"adapter\":{},\"backend\":{},\"software_gpu\":{},\"tile_id\":{},\"tile_uploads\":{},\"tile_upload_bytes\":{},\"retained_cpu_tile_bytes\":0,\"cache_entries\":1,\"uniform_uploads\":{},\"uniform_upload_bytes\":{},\"surface_reconfigurations\":{},\"interactions\":{},\"viewer_drags\":{},\"globe_drags\":{},\"clip_enabled\":{},\"scale_changes\":{},\"pixels_per_point\":{},\"native_pixels_per_point\":{},\"zoom_factor\":{},\"pointer\":{},\"pointer_down\":{},\"controls\":{{\"interact\":{},\"clip\":{},\"scale\":{},\"capture\":{}}},\"capture\":{{\"completed\":{},\"error\":{},\"viewer\":{},\"globe\":{}}},\"viewer\":{},\"globe\":{}}}",
            self.sequence,
            ready,
            json_string(&self.adapter),
            json_string(&self.backend),
            self.software_gpu,
            json_string(TILE_ID),
            metrics.tile_uploads,
            metrics.tile_upload_bytes,
            metrics.uniform_uploads,
            metrics.uniform_upload_bytes,
            metrics.surface_reconfigurations,
            ui.interactions,
            ui.drag_counts[0],
            ui.drag_counts[1],
            ui.clip_enabled,
            ui.scale_changes,
            context.pixels_per_point(),
            context.native_pixels_per_point().unwrap_or(1.0),
            context.zoom_factor(),
            pointer,
            pointer_down,
            point(ui.controls.interact),
            point(ui.controls.clip),
            point(ui.controls.scale),
            point(ui.controls.capture),
            capture.completed,
            capture_error.unwrap_or_else(|| "null".to_owned()),
            capture_view_json(&capture.viewer),
            capture_view_json(&capture.globe),
            viewport_json(viewer, ui.viewports[0]),
            viewport_json(globe, ui.viewports[1]),
        );
        emit(json);
        ready
    }

    pub fn shutdown(&self) {
        if self.enabled {
            emit("VELDMAP_SHUTDOWN".to_owned());
        }
    }
}

fn capture_view_json(view: &CaptureView) -> String {
    format!(
        "{{\"samples\":{},\"colors\":{},\"red\":{},\"green\":{},\"blue\":{},\"yellow\":{}}}",
        view.samples, view.colors, view.red, view.green, view.blue, view.yellow
    )
}

fn viewport_json(metrics: &ViewportSnapshot, ui: ViewportUi) -> String {
    format!(
        "{{\"draws\":{},\"tile_handle\":{},\"viewport_px\":[{:?},{:?}],\"clip_px\":[{:?},{:?}],\"screen_px\":[{:?},{:?}],\"pixels_per_point\":{},\"clip_modes\":{},\"size_changes\":{},\"rect\":{},\"input\":{}}}",
        metrics.draws,
        metrics.tile_handle,
        metrics.viewport_px[0],
        metrics.viewport_px[1],
        metrics.clip_px[0],
        metrics.clip_px[1],
        metrics.screen_px[0],
        metrics.screen_px[1],
        metrics.pixels_per_point,
        metrics.clip_modes,
        metrics.size_changes,
        numbers(&ui.rect),
        point(ui.input),
    )
}

fn point(point: [f32; 2]) -> String {
    numbers(&point)
}

fn numbers(values: &[f32]) -> String {
    let values = values
        .iter()
        .map(|value| format!("{value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(escaped, "\\u{:04x}", character as u32).expect("JSON escape");
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(not(target_arch = "wasm32"))]
fn enabled() -> bool {
    std::env::var_os("VELDMAP_SMOKE").is_some()
}

#[cfg(target_arch = "wasm32")]
fn enabled() -> bool {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| {
            query
                .split('&')
                .any(|part| part == "?smoke=1" || part == "smoke=1")
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn emit(line: String) {
    println!("{line}");
}

#[cfg(target_arch = "wasm32")]
fn emit(line: String) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&line));
    if let Some(output) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("smoke-status"))
    {
        output.set_text_content(Some(&line));
    }
}
