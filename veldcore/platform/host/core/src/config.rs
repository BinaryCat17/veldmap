use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use regex::Regex;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceEntry {
    pub location: String,
    pub path: Option<String>,
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServicesManifest {
    pub services: HashMap<String, ServiceEntry>,
    pub logs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub project_root: std::path::PathBuf,
    pub manifest: ServicesManifest,
    pub plugin_configs: HashMap<String, HashMap<String, serde_json::Value>>,
    pub plugin_raw_configs: HashMap<String, String>,
}

pub fn load_host_config(config_dir: &str) -> anyhow::Result<HostConfig> {
    let manifest_path = Path::new(config_dir).join("services.json");
    let project_root = Path::new(config_dir).parent().unwrap_or(Path::new(".")).to_path_buf();
    
    let manifest: ServicesManifest = if manifest_path.exists() {
        load_config_with_path(&manifest_path)?
    } else {
        ServicesManifest { services: HashMap::new(), logs: None }
    };
    
    let mut plugin_configs = HashMap::new();
    let mut plugin_raw_configs = HashMap::new();
    
    for (name, entry) in &manifest.services {
        if entry.location == "local" {
            let service_config_path = Path::new(config_dir).join(format!("{}.json", name));
            let service_config_str = read_config_string(&service_config_path).unwrap_or_else(|_| "{}".to_string());
            
            if let Ok(config_map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&service_config_str) {
                plugin_configs.insert(name.clone(), config_map);
            }
            plugin_raw_configs.insert(name.clone(), service_config_str);
        }
    }
    
    Ok(HostConfig {
        project_root,
        manifest,
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
        std::env::var(var_name).unwrap_or_else(|_| String::new())
    }).to_string()
}
