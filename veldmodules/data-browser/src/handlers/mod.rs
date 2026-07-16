//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod download;
pub mod nav;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_screen: Option<String>,
}
