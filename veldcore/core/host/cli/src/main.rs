use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
    CallContext,
};
use std::sync::Arc;
use extism::{Function, UserData, Val, ValType, CurrentPlugin};
use extism_convert::MemoryHandle;
use veldmap_host_core::services::{RpcRequest, RpcResponse};
use prost::Message;
use serde_json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info,veldmap_host_cli=info,veldmap_host_core=info");
    }
    env_logger::init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap CLI Host starting (config: {})...", config_dir);

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
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(Arc::new(device), Arc::new(queue)));

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone()))));

    let resources_for_factory = resources.clone();
    let dispatcher_for_factory = dispatcher.clone();

    let factory = Box::new(move |p_name: &str, config_map: &std::collections::HashMap<String, serde_json::Value>| {
        let mut host_functions = Vec::new();
        let d_inner = dispatcher_for_factory.clone();
        let plugin_name = p_name.to_string();

        let p_name_call = plugin_name.clone();
        let mut veld_host_call = Function::new("veld_host_call", [ValType::I64, ValType::I64], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let wasm_ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(wasm_ptr, len) };
                let req_buf = plugin.memory_bytes(handle)?;
                let request = RpcRequest::decode(req_buf)?;
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(d_inner.call(&request.service, &request.method, request.payload))
                });
                let (payload, error) = match result { 
                    Ok(p) => (p, String::new()), 
                    Err(e) => {
                        eprintln!("[ABI:{}] Service call failed: {}", p_name_call, e);
                        (Vec::new(), e.to_string())
                    }
                };
                let res_buf = RpcResponse { payload, error, sync: None }.encode_to_vec();
                let res_mem = plugin.memory_new(&res_buf)?;
                outputs[0] = Val::I64(res_mem.offset() as i64);
                Ok(())
            }
        );
        veld_host_call.set_namespace("env");
        host_functions.push(veld_host_call);

        let res_gpu_w = resources_for_factory.clone();
        let mut veld_gpu_write = Function::new("veld_gpu_write", [ValType::I64, ValType::I64, ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let res_id = inputs[0].i64().unwrap() as u64;
                let offset = inputs[1].i64().unwrap() as u64;
                let ptr = inputs[2].i64().unwrap() as u64;
                let size = inputs[3].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, size) };
                let data = plugin.memory_bytes(handle)?;
                res_gpu_w.write_resource(res_id, offset, data).map_err(|e| extism::Error::msg(e.to_string()))?;
                Ok(())
            }
        );
        veld_gpu_write.set_namespace("env");
        host_functions.push(veld_gpu_write);

        let res_gpu_r = resources_for_factory.clone();
        let mut veld_gpu_read = Function::new("veld_gpu_read", [ValType::I64, ValType::I64, ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let res_id = inputs[0].i64().unwrap() as u64;
                let offset = inputs[1].i64().unwrap() as u64;
                let ptr = inputs[2].i64().unwrap() as u64;
                let size = inputs[3].i64().unwrap() as u64;
                let data = res_gpu_r.read_resource(res_id, offset, size).map_err(|e| extism::Error::msg(e.to_string()))?;
                let handle = unsafe { MemoryHandle::new(ptr, size) };
                let wasm_mem = plugin.memory_bytes_mut(handle)?;
                wasm_mem[..data.len()].copy_from_slice(&data);
                Ok(())
            }
        );
        veld_gpu_read.set_namespace("env");
        host_functions.push(veld_gpu_read);

        let config_clone = config_map.clone();
        let mut veld_get_info = Function::new("veld_get_info", [ValType::I64, ValType::I64], [ValType::I64], UserData::new(config_clone),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], user_data: UserData<std::collections::HashMap<String, serde_json::Value>>| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, len) };
                let key_bytes = plugin.memory_bytes(handle)?;
                let key = std::str::from_utf8(key_bytes).map_err(|_| extism::Error::msg("Invalid UTF-8 key"))?;
                
                let config_res = user_data.get()?;
                let config = config_res.lock().unwrap();
                match config.get(key) {
                    Some(val) => {
                        let s: String = if let Some(s) = val.as_str() { s.to_string() } else { val.to_string() };
                        let mem = plugin.memory_new(s.as_str())?;
                        outputs[0] = Val::I64(mem.offset() as i64);
                    }
                    None => {
                        outputs[0] = Val::I64(0);
                    }
                }
                Ok(())
            }
        );
        veld_get_info.set_namespace("env");
        host_functions.push(veld_get_info);

        let mut v_ptr_len = Function::new("veld_ptr_len", [ValType::I64], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = plugin.memory_length(ptr)?;
                outputs[0] = Val::I64(len as i64);
                Ok(())
            }
        );
        v_ptr_len.set_namespace("env");
        host_functions.push(v_ptr_len);

        let mut v_load_u8 = Function::new("veld_load_u8", [ValType::I64], [ValType::I32], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, 1) };
                let b = plugin.memory_bytes(handle)?[0];
                outputs[0] = Val::I32(b as i32);
                Ok(())
            }
        );
        v_load_u8.set_namespace("env");
        host_functions.push(v_load_u8);

        let mut v_input_len = Function::new("veld_input_len", [], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, _inputs: &[Val], outputs: &mut [Val], _| {
                let ctx: CallContext = plugin.host_context::<CallContext>()?.clone();
                let inner = ctx.0.lock().unwrap();
                outputs[0] = Val::I64(inner.input.len() as i64);
                Ok(())
            }
        );
        v_input_len.set_namespace("env");
        host_functions.push(v_input_len);

        let mut v_input_load_u8 = Function::new("veld_input_load_u8", [ValType::I64], [ValType::I32], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let idx = inputs[0].i64().unwrap() as usize;
                let ctx: CallContext = plugin.host_context::<CallContext>()?.clone();
                let inner = ctx.0.lock().unwrap();
                let b = inner.input[idx];
                outputs[0] = Val::I32(b as i32);
                Ok(())
            }
        );
        v_input_load_u8.set_namespace("env");
        host_functions.push(v_input_load_u8);

        let mut v_output_set = Function::new("veld_output_set", [ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, len) };
                let data = plugin.memory_bytes(handle)?.to_vec();
                let ctx: CallContext = plugin.host_context::<CallContext>()?.clone();
                let mut inner = ctx.0.lock().unwrap();
                inner.output = data;
                Ok(())
            }
        );
        v_output_set.set_namespace("env");
        host_functions.push(v_output_set);

        let mut v_alloc = Function::new("veld_alloc", [ValType::I64], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let len = inputs[0].i64().unwrap() as u64;
                let mem = plugin.memory_alloc(len)?;
                outputs[0] = Val::I64(mem.offset() as i64);
                Ok(())
            }
        );
        v_alloc.set_namespace("env");
        host_functions.push(v_alloc);

        let mut v_free = Function::new("veld_free", [ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, 0) };
                plugin.memory_free(handle)?;
                Ok(())
            }
        );
        v_free.set_namespace("env");
        host_functions.push(v_free);

        let mut veld_http_request = Function::new("veld_http_request", [ValType::I64, ValType::I64, ValType::I64, ValType::I64], [ValType::I64], UserData::new(()),
            move |_plugin: &mut CurrentPlugin, _inputs: &[Val], outputs: &mut [Val], _| {
                outputs[0] = Val::I64(0);
                Ok(())
            }
        );
        veld_http_request.set_namespace("env");
        host_functions.push(veld_http_request);

        let mut veld_http_status_get = Function::new("veld_http_status_get", [], [ValType::I32], UserData::new(()),
            move |_plugin: &mut CurrentPlugin, _inputs: &[Val], outputs: &mut [Val], _| {
                outputs[0] = Val::I32(200);
                Ok(())
            }
        );
        veld_http_status_get.set_namespace("env");
        host_functions.push(veld_http_status_get);

        host_functions
    });

    plugin_module::load_services(dispatcher.clone(), &config_dir, factory).await?;

    let node = Arc::new(VeldmapNode::new(endpoint, dispatcher.clone()).await?);
    log::info!("Node ID: {}", node.node_id());

    node.run().await?;

    Ok(())
}