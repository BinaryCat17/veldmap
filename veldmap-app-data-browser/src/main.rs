pub mod common;
pub mod search;
pub mod browse;
pub mod downloaded;
pub mod preview;
pub mod utils;
pub mod app;

use iced::{Settings, Font, Pixels};
use crate::app::VeldMapToolsGui;
use veldmap_data_provider::CdseConfig;
use std::fs::File;
use std::os::unix::io::AsRawFd;

fn main() -> iced::Result {
    // Logger initialization
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    // Redirect stdio to avoid broken pipe crashes on Linux
    let _ = std::fs::create_dir_all("cache");
    if let Ok(log_file) = File::create("cache/veldmap-browser.log") {
        let fd = log_file.as_raw_fd();
        unsafe {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }

    // Read config from environment
    let config = CdseConfig {
        access_key: std::env::var("CDSE_ACCESS_KEY").unwrap_or_default(),
        secret_key: std::env::var("CDSE_SECRET_KEY").unwrap_or_default(),
        region: "eu-central-1".to_string(),
        endpoint: "https://eodata.dataspace.copernicus.eu".to_string(),
    };

    let (app, task) = VeldMapToolsGui::new(config);

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