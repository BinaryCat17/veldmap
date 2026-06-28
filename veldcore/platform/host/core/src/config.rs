use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;
use regex::Regex;

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
