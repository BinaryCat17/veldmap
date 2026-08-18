//! Window configuration for plugins
//! 
//! Each plugin can request a window by specifying "window" in its config.
//! The host creates and manages windows for plugins.

use serde::Deserialize;

/// Окно, которое просит модуль.
#[derive(Deserialize, Clone, Debug)]
pub struct PluginWindowConfig {
    /// Заголовок окна.
    #[serde(default = "default_title")]
    pub title: String,
    
    /// Ширина в логических пикселях.
    #[serde(default = "default_width")]
    pub width: u32,
    
    /// Высота в логических пикселях.
    #[serde(default = "default_height")]
    pub height: u32,
    
    /// Масштаб интерфейса. Нижняя граница: хост шлёт
    /// модулям `max(window.scale_factor(), ui_scale)`, т.к. на X11/WSLg winit
    /// часто репортит 1.0 даже на HiDPI-экранах.
    #[serde(default = "default_scale")]
    pub ui_scale: f32,
    
    /// Можно ли тянуть за край.
    #[serde(default = "default_resizable")]
    pub resizable: bool,
    
    /// Открывать ли во весь экран.
    #[serde(default)]
    pub fullscreen: bool,

    /// Где открыть. `None` — посередине экрана.
    pub position: Option<WindowPosition>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
}

fn default_title() -> String {
    "VeldMap".to_string()
}

fn default_width() -> u32 {
    1024
}

fn default_height() -> u32 {
    768
}

fn default_scale() -> f32 {
    1.0
}

fn default_resizable() -> bool {
    true
}

/// Окна, объявленные модулями: каждый вправе попросить своё ключом `window`
/// в конфиге. Возвращаются парами «владелец — что просил».
pub fn extract_window_configs(config: &veldmap_host_core::config::HostConfig) -> Vec<(String, PluginWindowConfig)> {
    let mut windows = Vec::new();

    for (name, plugin_config) in &config.plugin_configs {
        if let Some(value) = plugin_config.get("window") {
            match serde_json::from_value::<PluginWindowConfig>(value.clone()) {
                Ok(window_config) => {
                    log::info!("Module '{}' declares window: {}x{}", name, window_config.width, window_config.height);
                    windows.push((name.clone(), window_config));
                }
                Err(e) => log::error!("Module '{}' has an invalid window declaration: {}", name, e),
            }
        }
    }

    windows.sort_by(|a, b| a.0.cmp(&b.0));
    windows
}
