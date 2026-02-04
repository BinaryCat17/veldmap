use std::sync::Arc;

pub mod config_module;
pub mod plugin_module;
pub mod dispatcher;
pub mod node;
pub mod system_service;
pub mod resources;

pub mod services {
    include!(concat!(env!("OUT_DIR"), "/veldmap.services.rs"));
}

pub mod ui {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
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

pub use config_module::*;
pub use plugin_module::*;
pub use dispatcher::*;
pub use node::*;
pub use system_service::*;