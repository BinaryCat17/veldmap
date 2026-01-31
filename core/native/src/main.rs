use veldmap_native_host::app_service::AppCommand;
use veldmap_native_host::dispatcher::Dispatcher;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    window::WindowBuilder,
};
use tokio::sync::mpsc;
use std::sync::{Arc};
use extism::{Function, UserData, Val, ValType};
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "error,veldmap_host=info,veldmap_native_host=info,veldmap_native_host::system_service=info");
    }
    env_logger::init();

    log::info!("VeldMap Native Host starting...");

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    
    let window = WindowBuilder::new()
        .with_title("VeldMap")
        .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0))
        .build(&event_loop)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AppCommand>();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    
    let surface = instance.create_surface(&window)?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }).await.ok_or_else(|| anyhow::anyhow!("Compatible GPU adapter not found."))?;
    
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;

    let caps = surface.get_capabilities(&adapter);
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: caps.formats[0],
        width: 1024,
        height: 768,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("system".to_string(), 
        veldmap_native_host::dispatcher::ServiceLocation::Native(Arc::new(veldmap_native_host::system_service::SystemService)));
    dispatcher.register_service("app".to_string(), 
        veldmap_native_host::dispatcher::ServiceLocation::Native(Arc::new(veldmap_native_host::app_service::AppService::new(tx))));

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

    veldmap_native_host::plugin_module::load_services_with_functions(dispatcher.clone(), vec![host_call]).await?;

    let d_clone = dispatcher.clone();
    let proxy_clone = proxy.clone();
    tokio::spawn(async move {
        // Задержка перед инициализацией приложения, чтобы окно точно было готово
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        
        let node = Arc::new(veldmap_native_host::node::VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });

        log::info!("Core ready. Launching App...");
        let _ = d_clone.call("veldmap-app-data-browser", "init", vec![]).await;
        let _ = proxy_clone.send_event(());
    });

    log::info!("VeldMap Ready.");

    event_loop.run(move |event: Event<()>, window_target: &EventLoopWindowTarget<()>| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(_) | Event::AboutToWait => {
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        AppCommand::Draw(data, _w, _h) => {
                            if data.len() < 4 { continue; }
                            let r = data[0] as f64 / 255.0;
                            let g = data[1] as f64 / 255.0;
                            let b = data[2] as f64 / 255.0;
                            
                            match surface.get_current_texture() {
                                Ok(frame) => {
                                    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                                    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                                    {
                                        let _rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                            label: None,
                                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                                view: &view,
                                                resolve_target: None,
                                                ops: wgpu::Operations {
                                                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                                                    store: wgpu::StoreOp::Store,
                                                },
                                            })],
                                            depth_stencil_attachment: None,
                                            ..Default::default()
                                        });
                                    }
                                    queue.submit(Some(encoder.finish()));
                                    frame.present();
                                }
                                Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                                    surface.configure(&device, &config);
                                }
                                Err(e) => log::error!("Surface error: {:?}", e),
                            }
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    config.width = size.width;
                    config.height = size.height;
                    surface.configure(&device, &config);
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                window_target.exit();
            }
            _ => (),
        }
    })?;

    Ok(())
}
