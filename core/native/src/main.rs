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
    let surface_format = caps.formats[0];
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: 1024,
        height: 768,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    // Подготовка ресурсов для отрисовки текстуры приложения
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Blit Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Blit Bind Group Layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Blit Pipeline Layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blit Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let mut app_texture: Option<wgpu::Texture> = None;
    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut last_size = (100u32, 100u32);

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
                let mut redraw_needed = false;
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        AppCommand::Draw(data, w, h) => {
                            if data.len() < (w * h * 4) as usize { continue; }
                            
                            if app_texture.is_none() || last_size != (w, h) {
                                let texture = device.create_texture(&wgpu::TextureDescriptor {
                                    label: Some("App Texture"),
                                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu::TextureFormat::Rgba8Unorm,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                                    view_formats: &[],
                                });
                                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some("App Bind Group"),
                                    layout: &bind_group_layout,
                                    entries: &[
                                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                                    ],
                                });
                                app_texture = Some(texture);
                                app_bind_group = Some(bind_group);
                                last_size = (w, h);
                            }

                            if let Some(texture) = &app_texture {
                                queue.write_texture(
                                    wgpu::TexelCopyTextureInfo {
                                        texture,
                                        mip_level: 0,
                                        origin: wgpu::Origin3d::ZERO,
                                        aspect: wgpu::TextureAspect::All,
                                    },
                                    &data,
                                    wgpu::TexelCopyBufferLayout {
                                        offset: 0,
                                        bytes_per_row: Some(4 * w),
                                        rows_per_image: Some(h),
                                    },
                                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                                );
                                redraw_needed = true;
                            }
                        }
                    }
                }

                if redraw_needed {
                    if let Some(bind_group) = &app_bind_group {
                        match surface.get_current_texture() {
                            Ok(frame) => {
                                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                                {
                                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: None,
                                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                            view: &view,
                                            resolve_target: None,
                                            ops: wgpu::Operations {
                                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                                store: wgpu::StoreOp::Store,
                                            },
                                        })],
                                        depth_stencil_attachment: None,
                                        ..Default::default()
                                    });
                                    rp.set_pipeline(&render_pipeline);
                                    rp.set_bind_group(0, bind_group, &[]);
                                    rp.draw(0..3, 0..1);
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
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    config.width = size.width;
                    config.height = size.height;
                    surface.configure(&device, &config);
                    
                    use veldmap_rust_rpc::ui::{UiEvent, ResizeEvent, ui_event};
                    let ev = UiEvent {
                        event: Some(ui_event::Event::Resize(ResizeEvent { width: size.width, height: size.height })),
                    };
                    let _ = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(dispatcher.call("veldmap-app-data-browser", "handle_ui_event", ev.encode_to_vec()))
                    });
                }
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { .. }, .. } => {
                // Игнорируем движение мыши для разгрузки WASM-плагина
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                use veldmap_rust_rpc::ui::{UiEvent, ClickEvent, ui_event};
                use winit::event::ElementState;
                if state == ElementState::Pressed {
                    let btn = match button {
                        winit::event::MouseButton::Left => 1,
                        winit::event::MouseButton::Right => 2,
                        winit::event::MouseButton::Middle => 3,
                        _ => 0,
                    };
                    let ev = UiEvent {
                        event: Some(ui_event::Event::Click(ClickEvent { x: -1.0, y: -1.0, button: btn })),
                    };
                    let d_clone = dispatcher.clone();
                    tokio::spawn(async move {
                        let _ = d_clone.call("veldmap-app-data-browser", "handle_ui_event", ev.encode_to_vec()).await;
                    });
                }
            }
            Event::WindowEvent { event: WindowEvent::KeyboardInput { event: key_event, .. }, .. } => {
                use veldmap_rust_rpc::ui::{UiEvent, KeyEvent, ui_event};
                let ev = UiEvent {
                    event: Some(ui_event::Event::Key(KeyEvent { 
                        key_code: 0,
                        pressed: key_event.state == winit::event::ElementState::Pressed 
                    })),
                };
                let d_clone = dispatcher.clone();
                tokio::spawn(async move {
                    let _ = d_clone.call("veldmap-app-data-browser", "handle_ui_event", ev.encode_to_vec()).await;
                });
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                window_target.exit();
            }
            _ => (),
        }
    })?;

    Ok(())
}
