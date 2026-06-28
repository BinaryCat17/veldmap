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

#[derive(serde::Deserialize)]
struct ServiceEntry {
    location: String,
    #[allow(dead_code)]
    node_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct ServicesManifest {
    services: std::collections::HashMap<String, ServiceEntry>,
}

/// Scan plugin configs for window preferences
pub fn scan_window_configs(config_dir: &str) -> anyhow::Result<PluginWindows> {
    let manifest_path = std::path::Path::new(config_dir).join("services.json");
    if !manifest_path.exists() {
        log::warn!("Manifest not found at {:?}", manifest_path);
        return Ok(PluginWindows::new());
    }

    let manifest: ServicesManifest = veldmap_host_core::config::load_config_with_path(&manifest_path)?;
    
    let mut windows = PluginWindows::new();

    for (name, entry) in manifest.services {
        if entry.location == "local" {
            let service_config_path = std::path::Path::new(config_dir).join(format!("{}.json", name));
            if let Ok(config) = veldmap_host_core::config::load_config_with_path::<serde_json::Value, _>(&service_config_path) {
                if let Some(window_config) = parse_window_config(&config) {
                        log::info!("Plugin '{}' requests window: {}x{} (scale: {})", 
                            name, window_config.width, window_config.height, window_config.ui_scale);
                        windows.add(name, window_config);
                }
            }
        }
    }
    
    Ok(windows)
}
