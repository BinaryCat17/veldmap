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

/// Parse window config from plugin's JSON config
pub fn parse_window_config(config: &serde_json::Value) -> Option<PluginWindowConfig> {
    config.get("window").and_then(|w| {
        serde_json::from_value(w.clone()).ok()
    })
}

/// Collection of window configs for all plugins
#[derive(Default)]
pub struct PluginWindows {
    configs: std::collections::HashMap<String, PluginWindowConfig>,
}

impl PluginWindows {
    pub fn new() -> Self {
        Self {
            configs: std::collections::HashMap::new(),
        }
    }
    
    pub fn add(&mut self, plugin_name: String, config: PluginWindowConfig) {
        self.configs.insert(plugin_name, config);
    }
    
    pub fn get(&self, plugin_name: &str) -> Option<&PluginWindowConfig> {
        self.configs.get(plugin_name)
    }
    
    pub fn has_windows(&self) -> bool {
        !self.configs.is_empty()
    }
    
    /// Get the first window config (for single-window mode)
    pub fn first(&self) -> Option<(&String, &PluginWindowConfig)> {
        self.configs.iter().next()
    }
}
