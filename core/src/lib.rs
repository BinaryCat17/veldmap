pub mod config_module;
pub mod plugin_module;
pub mod dispatcher;
pub mod node;

use std::sync::Arc;

#[global_allocator]
static GLOBAL: std::alloc::System = std::alloc::System;

pub async fn bootstrap() -> anyhow::Result<Arc<crate::node::VeldmapNode>> {
    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![b"veldmap/rpc/1".to_vec()])
        .bind()
        .await?;
        
    let dispatcher = Arc::new(crate::dispatcher::Dispatcher::new(endpoint.clone()));
    
    // Загружаем плагины
    plugin_module::load_services(dispatcher.clone()).await?;
    
    let node = Arc::new(crate::node::VeldmapNode::new(endpoint, dispatcher).await?);
    
    Ok(node)
}

pub use config_module::*;
pub use plugin_module::*;
pub use dispatcher::*;
pub use node::*;