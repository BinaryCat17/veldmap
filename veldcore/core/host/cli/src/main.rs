use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn,veldmap_host=info,veldmap_host_cli=info,veldmap_host_core=info,wasm=info,iroh=error,iroh_gossip=error,wasmtime_wasi=error,wgpu_core=error,wgpu_hal=error,sctk=error");
    }
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            use std::io::Write;
            let ts = buf.timestamp();
            let level = record.level();
            let target = record.target();
            let args = record.args();

            if target == "wasm" {
                writeln!(buf, "[{} {:5}] {}", ts, level, args)
            } else if target.starts_with("veldmap") {
                writeln!(buf, "[{} {:5}] [host] {}", ts, level, args)
            } else {
                writeln!(buf, "[{} {:5}] <{}> {}", ts, level, target, args)
            }
        })
        .init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap CLI Host starting...");

    let flags = wgpu::InstanceFlags::default() 
        | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER
        | wgpu::InstanceFlags::DEBUG
        | wgpu::InstanceFlags::VALIDATION;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        flags,
        ..Default::default()
    });
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { ..Default::default() }).await
        .expect("No WGPU adapter found");
    
    let info = adapter.get_info();
    log::info!("Using headless GPU adapter: {} ({:?})", info.name, info.backend);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await?;
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(
        Arc::new(device), 
        Arc::new(std::sync::Mutex::new(queue)),
        wgpu::TextureFormat::Rgba8Unorm
    ));

    let secret_key = iroh::SecretKey::generate(&mut rand::rng());
    let endpoint = iroh::Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![b"veldmap/rpc/1".to_vec()])
        .bind()
        .await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone(), dispatcher.tasks.clone()))));

    plugin_module::load_services(dispatcher.clone(), resources.clone(), &config_dir).await?;

    let node = Arc::new(VeldmapNode::new(endpoint, dispatcher.clone()).await?);
    log::info!("Node ID: {}", node.node_id());

    node.run().await?;

    Ok(())
}
