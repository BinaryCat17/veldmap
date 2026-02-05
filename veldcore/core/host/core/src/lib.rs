#![recursion_limit = "512"]
use std::sync::Arc;

pub mod config_module;
pub mod plugin_module;
pub mod abi;
pub mod dispatcher;
pub mod node;
pub mod system_service;
pub mod gpu_service;
pub mod resources;

pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod wgpu {
    include!(concat!(env!("OUT_DIR"), "/veldmap.wgpu.rs"));
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
    pub resources: Arc<crate::resources::ResourceManager>,
    pub plugin_name: String,
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
pub use system_service::*;