use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
};
use crate::app_service::{AppCommand, AppService};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    window::WindowBuilder,
};
use tokio::sync::mpsc;
use std::sync::Arc;
use extism::{Function, UserData, Val, ValType};
use veldmap_host_core::services::{RpcRequest, RpcResponse};
use prost::Message;

mod app_service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "error,veldmap_host=info,veldmap_host_gui=info,veldmap_host_core=info");
    }
    env_logger::init();

    let mut config_dir = "config".to_string();
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            config_dir = args[i + 1].clone();
        }
    }

    log::info!("VeldMap GUI Host starting (config: {})...", config_dir);

    let event_loop = EventLoopBuilder::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    
    let window = Arc::new(WindowBuilder::new()
        .with_title("VeldMap")
        .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0))
        .build(&event_loop)?);

    let (tx, mut rx) = mpsc::unbounded_channel::<AppCommand>();

    // ... (WGPU initialization code) ...
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let surface = instance.create_surface(window.clone())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }).await.ok_or_else(|| anyhow::anyhow!("Compatible GPU adapter not found."))?;
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats[0];
    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Blit Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
    });

    // ... (Pipeline initialization) ...
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Blit Bind Group Layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&bind_group_layout], push_constant_ranges: &[] });
    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blit Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), targets: &[Some(wgpu::ColorTargetState { format: surface_format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
        depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });

    let mut app_texture: Option<wgpu::Texture> = None;
    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut last_size = (100u32, 100u32);
    let mut cursor_pos = (0.0f32, 0.0f32);
    let is_occluded = false;

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService)));
    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(tx, proxy))));

    let d_call = dispatcher.clone();
    let mut host_call = Function::new("veldmap_host_call", [ValType::I64], [ValType::I64], UserData::new(()),
        move |plugin, inputs, outputs, _| {
            let offset = inputs[0].i64().unwrap_or(0) as u64;
            if offset == 0 { return Err(extism::Error::msg("Guest passed null pointer")); }

            let req_buf: Vec<u8> = plugin.memory_get_val(&inputs[0])?;
            let request = RpcRequest::decode(&req_buf[..])?;
            
            // Выполняем вызов сервиса. 
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(d_call.call(&request.service, &request.method, request.payload))
            });

            let (payload, error) = match result { 
                Ok(p) => (p, String::new()), 
                Err(e) => {
                    log::warn!("Host service error ({}::{}): {}", request.service, request.method, e);
                    (Vec::new(), e.to_string())
                }
            };

            let res_buf = RpcResponse { 
                payload, 
                error, 
                sync: Some(veldmap_host_core::services::SyncMetadata::default()) 
            }.encode_to_vec();
            
            let res_mem = plugin.memory_new(&res_buf)?;
            outputs[0] = Val::I64(res_mem.offset() as i64);
            Ok(())
        }
    );
    host_call.set_namespace("env");

    plugin_module::load_services_with_functions(dispatcher.clone(), vec![host_call], &config_dir).await?;

    let d_clone = dispatcher.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let node = Arc::new(VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });
        log::info!("Core ready. Heartbeat started...");
        
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;
            if let Err(e) = d_clone.call("data-browser", "render", vec![]).await {
                log::error!("Render call failed: {}", e);
            }
        }
    });

    event_loop.run(move |event: Event<()>, window_target: &EventLoopWindowTarget<()>| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(_) | Event::AboutToWait => {
                let mut last_draw_cmd = None;
                while let Ok(cmd) = rx.try_recv() { last_draw_cmd = Some(cmd); }

                if let Some(AppCommand::Draw(data, w, h)) = last_draw_cmd {
                    if !is_occluded && w > 0 && h > 0 {
                        if (w, h) != last_size || app_texture.is_none() {
                            let texture = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
                            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &bind_group_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }] });
                            app_texture = Some(texture);
                            app_bind_group = Some(bind_group);
                            last_size = (w, h);
                        }
                        if let Some(texture) = &app_texture {
                            queue.write_texture(wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
                            window.request_redraw();
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                if !is_occluded && config.width > 0 && config.height > 0 {
                    if let Some(bind_group) = &app_bind_group {
                        if let Ok(frame) = surface.get_current_texture() {
                            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                            {
                                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, ..Default::default() });
                                rp.set_pipeline(&render_pipeline);
                                rp.set_bind_group(0, bind_group, &[]);
                                rp.draw(0..3, 0..1);
                            }
                            queue.submit(Some(encoder.finish()));
                            log::trace!("Presenting frame...");
                            frame.present();
                            log::trace!("Frame presented.");
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    config.width = size.width; config.height = size.height;
                    surface.configure(&device, &config);
                    window.request_redraw();
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Resize(veldmap_host_core::ui::ResizeEvent { width: size.width, height: size.height, scale_factor: window.scale_factor() as f32 })) };
                    let d_clone = dispatcher.clone();
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; });
                }
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                if state == winit::event::ElementState::Pressed {
                    let btn = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Click(veldmap_host_core::ui::ClickEvent { x: cursor_pos.0, y: cursor_pos.1, button: btn })) };
                    let d_clone = dispatcher.clone();
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; });
                }
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => { window_target.exit(); }
            _ => (),
        }
    })?;

    Ok(())
}
