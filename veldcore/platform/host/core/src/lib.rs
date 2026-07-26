#![recursion_limit = "512"]
use std::sync::Arc;
use crate::dispatcher::Dispatcher;

pub mod registry;
pub mod memory;
pub mod graphics;
pub mod config;
pub mod plugins;
pub mod setup;
pub mod abi;
pub mod dispatcher;
pub mod logging;
pub mod tasks;
pub mod surfaces;

// Protobuf-типы платформенных контрактов — из сгенерированного
// биндинг-крейта (platform/host/generated, buildgen).
pub use veldmap_host_bindings::proto::{app, core};

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

/// Конфигурация core модуля
#[derive(serde::Deserialize, Debug)]
pub struct CoreConfig {
    /// Фильтр логов в синтаксисе env_logger, например
    /// "veldmap=info,veldmap::host::render=debug,veldmap::ui-service::handlers=trace".
    /// Подсистема — это таргет записи; RUST_LOG переопределяет значение отсюда.
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
    /// Минимальный интервал между одинаковыми логами в миллисекундах (0 = без ограничения)
    #[serde(default = "default_log_rate_limit_ms")]
    pub log_rate_limit_ms: u64,
}

fn default_log_rate_limit_ms() -> u64 {
    1000 // По умолчанию 1 секунда
}

/// Свои логи целиком, чужие крейты — только warn+.
/// netlink_packet_route на каждом старте пишет бесполезное «ядро новее крейта».
fn default_log_filter() -> String {
    "veldmap=trace,netlink_packet_route=error,warn".to_string()
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            log_filter: default_log_filter(),
            log_rate_limit_ms: default_log_rate_limit_ms(),
        }
    }
}
