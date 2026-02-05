use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
};
use std::sync::Arc;
use serde_json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info,veldmap_host_cli=info,veldmap_host_core=info,wgpu_core=warn,wgpu_hal=warn,naga=warn,iroh=warn,wasmtime_wasi=warn,cranelift_codegen=warn,tracing=warn");
    }
    env_logger::init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap CLI Host starting (DEBUG MODE)...");

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
        ..Default::default()
    });
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { ..Default::default() }).await
        .ok_or_else(|| anyhow::anyhow!("No WGPU adapter found"))?;
    
    let info = adapter.get_info();
    log::info!("Using headless GPU adapter: {} ({:?}, driver: {})", info.name, info.backend, info.driver);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(
        Arc::new(device), 
        Arc::new(queue),
        wgpu::TextureFormat::Rgba8Unorm
    ));

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone()))));

    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir).await?;

    let node = Arc::new(VeldmapNode::new(endpoint, dispatcher.clone()).await?);
    log::info!("Node ID: {}", node.node_id());

    node.run().await?;

    Ok(())
}
