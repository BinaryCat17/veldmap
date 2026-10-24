pub mod search;
pub mod browse;
pub mod globe;
pub mod library;
pub mod listing;
pub mod nav;
pub mod outline;
pub mod persist;
pub mod overlay;
pub mod preview;
pub mod window;

use veldsdk::graphics::TextureFormat;

pub fn open_resource(identifier: String, local: Option<String>, correlation: &str) {
    match local {
        Some(name) => crate::calls::data_library::on_open(
            &crate::proto::data_library::OpenRequest { name },
            correlation,
        ),
        None => crate::calls::data_provider::on_open(
            &crate::proto::data_provider::OpenRequest { identifier },
            correlation,
        ),
    }
}

pub const SURFACE_FORMAT: TextureFormat = TextureFormat::TexRgba8Unorm;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_view: Option<String>,
}
