#![recursion_limit = "512"]
use std::sync::Arc;

pub mod registry;
pub mod memory;
pub mod graphics;
pub mod config;
pub mod plugins;
pub mod setup;
pub mod abi;
pub mod dispatcher;
pub mod logging;

pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub struct CallContextInner {
    pub input: Vec<u8>,
    pub output: Vec<u8>,
}

#[derive(Clone)]
pub struct CallContext(pub Arc<std::sync::Mutex<CallContextInner>>);

impl CallContext {
    pub fn new(input: Vec<u8>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(CallContextInner { input, output: Vec::new() })))
    }
}

pub struct HostState {
    pub dispatcher: Arc<Dispatcher>,
    pub registry: Arc<crate::registry::ResourceRegistry>,
    pub memory: Arc<crate::memory::MemoryManager>,
    pub graphics: Arc<crate::graphics::GraphicsDevice>,
    pub plugin_name: String,
    pub instance_id: u32,
    pub config: std::collections::HashMap<String, serde_json::Value>,
    pub call_context: Option<CallContext>,
    pub wasi: wasmtime_wasi::p1::WasiP1Ctx,
    pub resource_limiter: wasmtime::StoreLimits,
}

pub struct WasmModule {
    pub store: wasmtime::Store<HostState>,
    pub instance: wasmtime::Instance,
}

pub use config::*;
pub use plugins::*;
pub use setup::*;
pub use dispatcher::*;
pub use graphics::*;
pub use memory::*;
pub use registry::*;

/// Конфигурация core модуля
#[derive(serde::Deserialize, Debug)]
pub struct CoreConfig {
    /// Флаги логирования - массив строк, например: ["DISPATCHER", "ABI", "HOST_RENDER"]
    #[serde(default, deserialize_with = "deserialize_log_flags")]
    pub log_flags: u32,
    /// Минимальный интервал между одинаковыми логами в миллисекундах (0 = без ограничения)
    #[serde(default = "default_log_rate_limit_ms")]
    pub log_rate_limit_ms: u64,
}

fn default_log_rate_limit_ms() -> u64 {
    1000 // По умолчанию 1 секунда
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            log_flags: 0,
            log_rate_limit_ms: default_log_rate_limit_ms(),
        }
    }
}

fn deserialize_log_flags<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    
    let flags: Vec<String> = Vec::deserialize(deserializer)?;
    let mut result: u32 = 0;
    
    for flag in flags {
        result |= match flag.as_str() {
            "PERF" => crate::logging::FLAG_PERF,
            "WASM" => crate::logging::FLAG_WASM,
            "DISPATCHER" => crate::logging::FLAG_DISPATCHER,
            "ABI" => crate::logging::FLAG_ABI,
            "HOST_RENDER" => crate::logging::FLAG_HOST_RENDER,
            "SDK" => crate::logging::FLAG_SDK,
            "UI_SERVICE" => crate::logging::FLAG_UI_SERVICE,
            "UI_HANDLERS" => crate::logging::FLAG_UI_HANDLERS,
            "GRAPHICS" => crate::logging::FLAG_GRAPHICS,
            _ => {
                eprintln!("Warning: unknown log flag '{}'", flag);
                0
            }
        };
    }
    
    Ok(result)
}
