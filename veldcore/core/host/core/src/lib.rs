pub mod config_module;
pub mod plugin_module;
pub mod dispatcher;
pub mod node;
pub mod system_service;

pub mod services {
    include!(concat!(env!("OUT_DIR"), "/veldmap.services.rs"));
}

pub mod ui {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
}

pub use config_module::*;
pub use plugin_module::*;
pub use dispatcher::*;
pub use node::*;
pub use system_service::*;