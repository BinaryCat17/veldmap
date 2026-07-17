//! Window configuration for plugins
//! 
//! Each plugin can request a window by specifying "window" in its config.
//! The host creates and manages windows for plugins.

use serde::Deserialize;

/// Window configuration that a plugin can request
#[derive(Deserialize, Clone, Debug)]
pub struct PluginWindowConfig {
    /// Window title
    #[serde(default = "default_title")]
    pub title: String,
    
    /// Window width in logical pixels
    #[serde(default = "default_width")]
    pub width: u32,
    
    /// Window height in logical pixels
    #[serde(default = "default_height")]
    pub height: u32,
    
    /// UI scale factor (DPI scaling)
    #[serde(default = "default_scale")]
    pub ui_scale: f32,
    
    /// Whether window should be resizable
    #[serde(default = "default_resizable")]
    pub resizable: bool,
    
    /// Whether window should be fullscreen
    #[serde(default)]
    pub fullscreen: bool,

    /// Window position (None = center on screen)
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

impl Default for PluginWindowConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            width: default_width(),
            height: default_height(),
            ui_scale: default_scale(),
            resizable: default_resizable(),
            fullscreen: false,
            position: None,
        }
    }
}

/// Collect window declarations from module configs: each module may declare
/// its own window under the "window" key. Returned as (owner, config) pairs.
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
