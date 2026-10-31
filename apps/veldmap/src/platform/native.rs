use eframe::egui;

use crate::app::VeldmapApp;

pub fn run() -> Result<(), eframe::Error> {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([640.0, 520.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = eframe::egui_wgpu::wgpu::Backends::VULKAN;
        setup.power_preference = eframe::egui_wgpu::wgpu::PowerPreference::HighPerformance;
        setup
            .instance_descriptor
            .flags
            .insert(eframe::egui_wgpu::wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER);
    }
    eframe::run_native(
        "VeldMap",
        options,
        Box::new(|creation| Ok(Box::new(VeldmapApp::new(creation)?))),
    )
}
