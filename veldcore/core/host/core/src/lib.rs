#![recursion_limit = "512"]
use std::sync::Arc;

pub mod config_module;
pub mod plugin_module;
pub mod abi;
pub mod dispatcher;
pub mod node;
pub mod resources;
pub mod logging;
pub mod window;

pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod compute {
    include!(concat!(env!("OUT_DIR"), "/veldmap.compute.rs"));
}

pub const SURFACE_ID: u64 = 0;

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
    pub resources: Arc<crate::resources::ResourceManager>,
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

pub use config_module::*;
pub use plugin_module::*;
pub use dispatcher::*;
pub use node::*;

/// Конфигурация core модуля
#[derive(serde::Deserialize, Debug, Default)]
pub struct CoreConfig {
    /// Флаги логирования - массив строк, например: ["DISPATCHER", "ABI", "HOST_RENDER"]
    #[serde(default, deserialize_with = "deserialize_log_flags")]
    pub log_flags: u32,
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
            "COMPUTE" => crate::logging::FLAG_COMPUTE,
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
