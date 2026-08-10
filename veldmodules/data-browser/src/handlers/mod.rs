//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod library;
pub mod nav;
pub mod preview;
pub mod window;

#[derive(serde::Deserialize)]
pub struct Config {
    /// Вид в стартовой вкладке: "search" (умолчание), "browse", "downloaded".
    /// Превью здесь нет: оно открывается на конкретный файл, а его в конфиге
    /// не назовёшь.
    pub initial_view: Option<String>,
}
