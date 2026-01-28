mod server;

use std::sync::Arc;
use veldmap_core::server_module::{VeldMapServer, ServerConfig};
use crate::server::VeldMapServerImpl;

/// Фабрика для создания экземпляра сервера.
pub fn create_server(config: ServerConfig) -> Arc<dyn VeldMapServer> {
    Arc::new(VeldMapServerImpl { config })
}
