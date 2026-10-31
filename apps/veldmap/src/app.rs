use std::sync::Arc;

use eframe::egui;
use thiserror::Error;
use veldmap_render::{
    GpuTile, RenderMetrics, SharedRenderResources, TileCallback, ViewportKind, ViewportTransform,
};

use crate::smoke::{ControlPoints, SmokeReporter, UiSnapshot, ViewportUi};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("eframe did not provide its wgpu render state")]
    MissingWgpu,
    #[cfg(target_arch = "wasm32")]
    #[error("WebGPU is required, but eframe selected {0}")]
    UnsupportedBackend(String),
}

pub struct VeldmapApp {
    tile: Arc<GpuTile>,
    metrics: Arc<RenderMetrics>,
    transforms: [ViewportTransform; 2],
    drag_counts: [u64; 2],
    clip_enabled: bool,
    interactions: u64,
    scale_changes: u64,
    gpu_warning: Option<String>,
    smoke: SmokeReporter,
}

impl VeldmapApp {
    pub fn new(creation: &eframe::CreationContext<'_>) -> Result<Self, AppError> {
        let render_state = creation
            .wgpu_render_state
            .as_ref()
            .ok_or(AppError::MissingWgpu)?;
        let adapter = render_state.adapter.get_info();
        #[cfg(target_arch = "wasm32")]
        if adapter.backend != eframe::egui_wgpu::wgpu::Backend::BrowserWebGpu {
            return Err(AppError::UnsupportedBackend(format!(
                "{:?}",
                adapter.backend
            )));
        }

        let software_gpu = adapter.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu;
        let gpu_warning = software_gpu.then(|| {
            if adapter.name.is_empty() {
                "Software GPU adapter selected".to_owned()
            } else {
                format!("Software GPU adapter: {}", adapter.name)
            }
        });
        if let Some(warning) = &gpu_warning {
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("VELDMAP_GPU_WARNING {warning}");
            #[cfg(target_arch = "wasm32")]
            web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(warning));
        }

        let metrics = Arc::new(RenderMetrics::default());
        let resources =
            SharedRenderResources::new(&render_state.device, render_state.target_format);
        let tile = GpuTile::upload_deterministic(
            &render_state.device,
            &render_state.queue,
            resources.tile_layout(),
            &metrics,
        );
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(resources);

        Ok(Self {
            tile,
            metrics,
            transforms: [ViewportTransform::default(), ViewportTransform::default()],
            drag_counts: [0, 0],
            clip_enabled: true,
            interactions: 0,
            scale_changes: 0,
            gpu_warning,
            smoke: SmokeReporter::new(adapter.name, format!("{:?}", adapter.backend), software_gpu),
        })
    }
}

impl eframe::App for VeldmapApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
        #[cfg(not(target_arch = "wasm32"))]
        let mut capture_requested = false;
        let mut ui_snapshot = UiSnapshot {
            interactions: self.interactions,
            drag_counts: self.drag_counts,
            clip_enabled: self.clip_enabled,
            scale_changes: self.scale_changes,
            ..Default::default()
        };

        egui::CentralPanel::default().show(root, |ui| {
            ui.horizontal(|ui| {
                let interact = ui.button("Interact");
                if interact.clicked() {
                    self.interactions += 1;
                }
                let clip = ui.checkbox(&mut self.clip_enabled, "Clip viewports");
                let scale = ui.button("Scale 125%");
                if scale.clicked() {
                    context.set_zoom_factor(1.25);
                    self.scale_changes += 1;
                }
                #[cfg(not(target_arch = "wasm32"))]
                let capture = ui.button("Capture");
                #[cfg(not(target_arch = "wasm32"))]
                {
                    capture_requested = capture.clicked();
                }
                ui.separator();
                ui.label("One RGBA tile, two geometries");
                ui_snapshot.controls = ControlPoints {
                    interact: response_center(&interact),
                    clip: response_center(&clip),
                    scale: response_center(&scale),
                    ..Default::default()
                };
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui_snapshot.controls.capture = response_center(&capture);
                }
            });
            ui.separator();
            ui.heading("Shared tile rendering");
            if let Some(warning) = &self.gpu_warning {
                ui.colored_label(egui::Color32::YELLOW, warning);
            }
            ui.label("Drag either geometry or use the wheel to zoom.");
            let tile = &self.tile;
            let metrics = &self.metrics;
            let transforms = &mut self.transforms;
            let drag_counts = &mut self.drag_counts;
            ui.columns(2, |columns| {
                for (index, kind) in ViewportKind::ALL.into_iter().enumerate() {
                    ui_snapshot.viewports[index] = paint_viewport(
                        &mut columns[index],
                        kind,
                        tile,
                        metrics,
                        &mut transforms[index],
                        &mut drag_counts[index],
                        self.clip_enabled,
                    );
                }
            });
            ui.separator();
            ui.label(format!(
                "Interactions: {} | Tile uploads: {} | Retained CPU pixels: 0 bytes",
                self.interactions,
                self.metrics.snapshot().tile_uploads
            ));
        });

        ui_snapshot.interactions = self.interactions;
        ui_snapshot.drag_counts = self.drag_counts;
        ui_snapshot.clip_enabled = self.clip_enabled;
        ui_snapshot.scale_changes = self.scale_changes;
        #[cfg(not(target_arch = "wasm32"))]
        if capture_requested {
            crate::capture::request(&context, &self.smoke.capture(), ui_snapshot);
        }
        let ready = self
            .smoke
            .report(&context, &self.metrics.snapshot(), &ui_snapshot);
        if self.smoke.enabled() && !ready {
            context.request_repaint();
        }
    }

    fn on_exit(&mut self) {
        self.smoke.shutdown();
    }
}

fn paint_viewport(
    ui: &mut egui::Ui,
    kind: ViewportKind,
    tile: &Arc<GpuTile>,
    metrics: &Arc<RenderMetrics>,
    transform: &mut ViewportTransform,
    drag_count: &mut u64,
    clip_enabled: bool,
) -> ViewportUi {
    ui.heading(kind.label());
    let size = egui::vec2(ui.available_width().max(160.0), 280.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if response.drag_started() {
        *drag_count += 1;
    }
    if response.dragged() {
        let delta = ui.input(|input| input.pointer.delta());
        match kind {
            ViewportKind::Viewer => {
                transform.pan[0] += delta.x * 2.0 / rect.width();
                transform.pan[1] -= delta.y * 2.0 / rect.height();
            }
            ViewportKind::Globe => {
                transform.rotation += delta.x * 0.006;
                transform.pan[1] -= delta.y * 2.0 / rect.height();
            }
        }
    }
    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll != 0.0 {
            transform.scale = (transform.scale + scroll * 0.001).clamp(0.3, 2.5);
        }
    }

    let clip_rect = if clip_enabled {
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.max.x, rect.min.y + rect.height() * 0.72),
        )
    } else {
        rect
    };
    ui.painter()
        .with_clip_rect(clip_rect)
        .add(eframe::egui_wgpu::Callback::new_paint_callback(
            rect,
            TileCallback::new(kind, Arc::clone(tile), *transform, Arc::clone(metrics)),
        ));
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::GRAY),
        egui::StrokeKind::Inside,
    );

    ViewportUi {
        rect: [rect.left(), rect.top(), rect.right(), rect.bottom()],
        input: point(clip_rect.center()),
    }
}

fn response_center(response: &egui::Response) -> [f32; 2] {
    point(response.rect.center())
}

fn point(point: egui::Pos2) -> [f32; 2] {
    [point.x, point.y]
}
