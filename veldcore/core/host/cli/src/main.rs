use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
};
use std::sync::Arc;
use extism::{Function, UserData, Val, ValType};
use veldmap_host_core::services::{RpcRequest, RpcResponse};
use prost::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    log::info!("VeldMap CLI Host starting (config: {})...", config_dir);

    let flags = wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags,
        ..Default::default()
    });
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { ..Default::default() }).await
        .ok_or_else(|| anyhow::anyhow!("No WGPU adapter found"))?;
    
    let info = adapter.get_info();
    log::info!("Using headless GPU adapter: {} ({:?}, driver: {})", info.name, info.backend, info.driver);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(Arc::new(device), Arc::new(queue)));

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone()))));

    let d_call = dispatcher.clone();
    let mut host_call = Function::new("veldmap_host_call", [ValType::I64], [ValType::I64], UserData::new(()),
        move |plugin, inputs, outputs, _| {
            let req_buf: Vec<u8> = plugin.memory_get_val(&inputs[0])?;
            let request = RpcRequest::decode(&req_buf[..])?;
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(d_call.call(&request.service, &request.method, request.payload))
            });
            let (payload, error) = match result { Ok(p) => (p, String::new()), Err(e) => (Vec::new(), e.to_string()) };
            let res_buf = RpcResponse { payload, error, sync: None }.encode_to_vec();
            let res_mem = plugin.memory_new(&res_buf)?;
            outputs[0] = Val::I64(res_mem.offset() as i64);
            Ok(())
        }
    );
    host_call.set_namespace("env");

    plugin_module::load_services_with_functions(dispatcher.clone(), vec![host_call], &config_dir).await?;

    let node = Arc::new(VeldmapNode::new(endpoint, dispatcher.clone()).await?);
    log::info!("Node ID: {}", node.node_id());

    // CLI просто держит ноду запущенной
    node.run().await?;

    Ok(())
}
