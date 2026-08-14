//! Handlers для data-browser

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

#[derive(serde::Deserialize)]
pub struct Config {
    /// Вид в стартовой вкладке: "search" (умолчание), "browse", "downloaded".
    /// Превью здесь нет: оно открывается на конкретный файл, а его в конфиге
    /// не назовёшь.
    ///
    /// Спрашивается только на первом запуске: дальше окно открывается тем, как
    /// его сложили в прошлый раз (см. handlers::persist).
    pub initial_view: Option<String>,
}
