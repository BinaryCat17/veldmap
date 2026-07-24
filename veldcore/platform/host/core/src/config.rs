use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use regex::Regex;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServicesManifest {
    /// Каталог с *.wasm плагинами, относительно `project_root`. По умолчанию
    /// `../build/plugins` (тот же путь, что раньше прописывался в
    /// services.json для каждого сервиса вручную).
    pub plugins_dir: Option<String>,
    pub logs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub project_root: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    pub plugins_dir: std::path::PathBuf,
    pub logs: Option<String>,
    pub plugin_configs: HashMap<String, HashMap<String, serde_json::Value>>,
    pub plugin_raw_configs: HashMap<String, String>,
}

/// services.json больше не перечисляет модули по имени (не источник истины —
/// см. `plugins::load_services`, где имя каждого плагина спрашивается у него
/// самого через ABI). Конфиг конкретного плагина ищется по имени файла
/// `<config_dir>/<имя>.json` в момент, когда имя уже известно; здесь же мы
/// заранее читаем все такие файлы (кроме зарезервированных services/core),
/// чтобы конфиги были доступны до загрузки wasm — они нужны, например, для
/// раннего определения окна (`window::extract_window_configs`).
pub fn load_host_config(config_dir: &str) -> anyhow::Result<HostConfig> {
    let config_dir_path = Path::new(config_dir).to_path_buf();
    let manifest_path = config_dir_path.join("services.json");
    let project_root = config_dir_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    let manifest: ServicesManifest = if manifest_path.exists() {
        load_config_with_path(&manifest_path)?
    } else {
        ServicesManifest { plugins_dir: None, logs: None }
    };

    let plugins_dir = project_root.join(manifest.plugins_dir.as_deref().unwrap_or("../build/plugins"));

    let mut plugin_configs = HashMap::new();
    let mut plugin_raw_configs = HashMap::new();

    if let Ok(entries) = fs::read_dir(&config_dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if stem == "services" || stem == "core" {
                continue;
            }
            let service_config_str = read_config_string(&path).unwrap_or_else(|_| "{}".to_string());
            if let Ok(config_map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&service_config_str) {
                plugin_configs.insert(stem.to_string(), config_map);
            }
            plugin_raw_configs.insert(stem.to_string(), service_config_str);
        }
    }

    Ok(HostConfig {
        project_root,
        config_dir: config_dir_path,
        plugins_dir,
        logs: manifest.logs,
        plugin_configs,
        plugin_raw_configs,
    })
}

pub fn load_config<T: DeserializeOwned>(crate_name: &str) -> anyhow::Result<T> {
    let mut path = std::env::current_dir()?;
    path.push("config");
    path.push(format!("{}.json", crate_name));
    load_config_with_path(path)
}

pub fn read_config_string<P: AsRef<Path>>(path: P) -> anyhow::Result<String> {
    let path = path.as_ref();
    
    if !path.exists() {
        return Err(anyhow::anyhow!("Config file not found: {:?}", path));
    }

    let content = fs::read_to_string(path)?;
    
    // Заменяем ${VAR} на значение из окружения
    let expanded = expand_env_vars(&content);
    Ok(expanded)
}

pub fn load_config_with_path<T: DeserializeOwned, P: AsRef<Path>>(path: P) -> anyhow::Result<T> {
    let expanded = read_config_string(path)?;
    let config: T = serde_json::from_str(&expanded)?;
    Ok(config)
}

fn expand_env_vars(text: &str) -> String {
    let re = Regex::new(r"\$\{([A-Za-z0-9_]+)\}").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_else(|_| {
            // Конфиг грузится до init_logging, поэтому предупреждение — в stderr.
            eprintln!("Warning: environment variable '{}' is not set, substituting empty string", var_name);
            String::new()
        })
    }).to_string()
}
