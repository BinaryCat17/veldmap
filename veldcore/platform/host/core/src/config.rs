use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use regex::Regex;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServicesManifest {
    /// Каталог с *.wasm плагинами, относительно `runtime_dir`. По умолчанию
    /// `../build/plugins`. Тот же каталог, что `plugins_dir` в workspace.yaml
    /// для сборки; build.py сверяет оба при каждой сборке, поэтому разойтись
    /// они не могут молча.
    pub plugins_dir: Option<String>,
    pub logs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Каталог runtime: родитель config_dir. База для относительных путей
    /// из конфигов (plugins_dir, logs) и из запросов модулей (см. util::path).
    pub runtime_dir: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    pub plugins_dir: std::path::PathBuf,
    pub logs: Option<String>,
    pub plugin_configs: HashMap<String, HashMap<String, serde_json::Value>>,
    pub plugin_raw_configs: HashMap<String, String>,
}

impl HostConfig {
    /// Файл лога — и умолчание к нему. Спрашивают его двое (логгер и раннер,
    /// кладущий снимки кадра рядом), поэтому путь собирается здесь.
    pub fn log_path(&self) -> std::path::PathBuf {
        self.runtime_dir.join(self.logs.as_deref().unwrap_or("logs/host.log"))
    }
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
    let runtime_dir = config_dir_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    let manifest: ServicesManifest = if manifest_path.exists() {
        load_config_with_path(&manifest_path)?
    } else {
        ServicesManifest { plugins_dir: None, logs: None }
    };

    let plugins_dir = runtime_dir.join(manifest.plugins_dir.as_deref().unwrap_or("../build/plugins"));

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
        runtime_dir,
        config_dir: config_dir_path,
        plugins_dir,
        logs: manifest.logs,
        plugin_configs,
        plugin_raw_configs,
    })
}

/// Подхватывает .env (KEY=VALUE) в окружение процесса. Уже заданные
/// переменные не переопределяются — как и у лаунчера, у окружения приоритет.
///
/// Читается до `load_host_config`, чтобы ${VAR} в конфигах раскрывался при
/// любом способе запуска. Разбирай .env только лаунчер — прямой запуск
/// бинарника молча получал бы пустые подстановки.
pub fn load_dotenv(path: &Path) {
    let Ok(content) = fs::read_to_string(path) else { return };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim();
        // Кавычки снимаем только парные ("..." или '...'), как в старом парсере.
        let bytes = value.as_bytes();
        let value = if value.len() >= 2
            && (bytes[0] == b'"' || bytes[0] == b'\'')
            && bytes[0] == bytes[value.len() - 1]
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        // Правка окружения безопасна только пока процесс однопоточен: другой
        // поток, читающий getenv в этот момент, получает гонку. Здесь это так —
        // .env читается до создания рантайма и загрузки сервисов.
        unsafe { std::env::set_var(key, value) };
    }
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
