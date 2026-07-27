//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod library;
pub mod nav;
pub mod preview;
pub mod window;

#[derive(serde::Deserialize)]
pub struct Config {
    pub initial_screen: Option<String>,
}

/// Имена UI-методов: приватная проводка между построением view (`.on_press`,
/// `.on_input`, ...) и диспетчером `on_ui_event` в module.rs. Не публичный
/// контракт — ui-service возвращает их эхом в `UiEventResponse.method`, сам
/// не зная и не проверяя их смысла.
pub mod ui_methods {
    pub const ON_NAV_BROWSE: &str = "on_nav_browse";
    pub const ON_NAV_SEARCH: &str = "on_nav_search";
    pub const ON_NAV_DOWNLOADED: &str = "on_nav_downloaded";
    pub const ON_BROWSE: &str = "on_browse";
    pub const ON_BROWSE_UP: &str = "on_browse_up";
    pub const ON_SEARCH: &str = "on_search";
    pub const ON_SEARCH_INPUT: &str = "on_search_input";
    pub const ON_DOWNLOAD_PRESSED: &str = "on_download_pressed";
    /// Просмотр скачанного файла (открывает data-library) и ещё не скачанного
    /// (открывает data-provider) — разные источники, дальше путь общий.
    pub const ON_VIEW_LOCAL_PRESSED: &str = "on_view_local_pressed";
    pub const ON_VIEW_REMOTE_PRESSED: &str = "on_view_remote_pressed";
    pub const ON_DELETE_PRESSED: &str = "on_delete_pressed";
}
