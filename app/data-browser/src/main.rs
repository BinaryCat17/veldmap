pub mod common;
pub mod search;
pub mod browse;
pub mod downloaded;
pub mod preview;
pub mod utils;
pub mod app;

use iced::{Settings, Font, Pixels};
use crate::app::VeldMapToolsGui;
use std::fs::File;
use std::os::unix::io::AsRawFd;

fn main() -> iced::Result {
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    let _ = std::fs::create_dir_all("cache");
    let _ = std::fs::create_dir_all("plugins");
    
    if let Ok(log_file) = File::create("cache/veldmap-browser.log") {
        let fd = log_file.as_raw_fd();
        unsafe {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }

    if let Err(e) = veldmap_core::init_plugins("plugins") {
        eprintln!("Plugin error: {}", e);
    }

    let (app, task) = VeldMapToolsGui::new();

    iced::application(
        "VeldMap Data Browser",
        VeldMapToolsGui::update,
        VeldMapToolsGui::view,
    )
    .settings(Settings {
        default_font: Font::DEFAULT,
        fonts: vec![include_bytes!("../../assets/NotoColorEmoji.ttf").into()],
        default_text_size: Pixels(14.0),
        ..Default::default()
    })
    .window(iced::window::Settings {
        size: iced::Size::new(1024.0, 768.0),
        ..Default::default()
    })
    .theme(VeldMapToolsGui::theme)
    .run_with(move || (app, task))
}