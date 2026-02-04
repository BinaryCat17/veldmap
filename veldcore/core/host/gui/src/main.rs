use veldmap_host_core::{
    dispatcher::{Dispatcher, ServiceLocation},
    node::VeldmapNode,
    plugin_module,
    system_service::SystemService,
    CallContext,
};
use crate::app_service::{AppCommand, AppService};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    window::WindowBuilder,
};
use tokio::sync::mpsc;
use std::sync::Arc;
use extism::{Function, UserData, Val, ValType, CurrentPlugin};
use extism_convert::MemoryHandle;
use veldmap_host_core::services::{RpcRequest, RpcResponse};
use prost::Message;

use std::sync::atomic::{AtomicBool, Ordering};

mod app_service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info,veldmap_host=debug,veldmap_host_gui=debug,veldmap_host_core=debug,wgpu_core=warn,wgpu_hal=warn,naga=warn,iroh=warn");
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
    let is_visible = Arc::new(AtomicBool::new(true));
    let ui_busy = Arc::new(AtomicBool::new(false));

    let flags = wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags,
        ..Default::default()
    });
    
    let surface = instance.create_surface(window.clone())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { 
        compatible_surface: Some(&surface), 
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default() 
    }).await.ok_or_else(|| anyhow::anyhow!("Compatible GPU adapter not found."))?;
    
    let info = adapter.get_info();
    log::info!("Using GPU adapter: {} ({:?}, driver: {}, backend: {:?})", info.name, info.device_type, info.driver, info.backend);

    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await?;
    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats[0];
    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    if size.width > 0 && size.height > 0 {
        surface.configure(&device, &config);
    }

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Blit Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
    });

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

    let device_arc = Arc::new(device);
    let queue_arc = Arc::new(queue);
    let resources = Arc::new(veldmap_host_core::resources::ResourceManager::new(device_arc.clone(), queue_arc.clone()));

    let mut app_texture_id: Option<u64> = None;
    let mut app_bind_group: Option<wgpu::BindGroup> = None;
    let mut last_size = (100u32, 100u32);
    let mut cursor_pos = (0.0f32, 0.0f32);
    let mut last_cursor_sent_time = std::time::Instant::now();

    let endpoint = iroh::Endpoint::builder().alpns(vec![b"veldmap/rpc/1".to_vec()]).bind().await?;
    let dispatcher = Arc::new(Dispatcher::new(endpoint.clone()));
    
    dispatcher.register_service("core".to_string(), ServiceLocation::Native(Arc::new(veldmap_host_core::dispatcher::CoreService)));
    dispatcher.register_service("system".to_string(), ServiceLocation::Native(Arc::new(SystemService::new(resources.clone()))));
    dispatcher.register_service("app".to_string(), ServiceLocation::Native(Arc::new(AppService::new(tx, proxy, is_visible.clone(), resources.clone()))));

    let resources_for_factory = resources.clone();
    let dispatcher_for_factory = dispatcher.clone();

    let factory = Box::new(move |p_name: &str, config_map: &std::collections::HashMap<String, serde_json::Value>| {
        let mut host_functions = Vec::new();
        let d_call_inner = dispatcher_for_factory.clone();
        let plugin_name = p_name.to_string();
        
        let p_name_call = plugin_name.clone();
        let mut veld_host_call = Function::new("veld_host_call", [ValType::I64, ValType::I64], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, len) };
                let req_buf = plugin.memory_bytes(handle)?;
                let request = RpcRequest::decode(req_buf)?;
                
                // eprintln!("[ABI:{}] Host call: {}.{}", p_name_call, request.service, request.method);
                
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(d_call_inner.call(&request.service, &request.method, request.payload))
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
        let p_name_w = plugin_name.clone();
        let mut veld_gpu_write = Function::new("veld_gpu_write", [ValType::I64, ValType::I64, ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let res_id = inputs[0].i64().unwrap() as u64;
                let offset = inputs[1].i64().unwrap() as u64;
                let ptr = inputs[2].i64().unwrap() as u64;
                let size = inputs[3].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, size) };
                let data = plugin.memory_bytes(handle)?;
                // eprintln!("[ABI:{}] GPU Write: id={}, size={}", p_name_w, res_id, size);
                res_gpu_w.write_resource(res_id, offset, data).map_err(|e| extism::Error::msg(e.to_string()))?;
                Ok(())
            }
        );
        veld_gpu_write.set_namespace("env");
        host_functions.push(veld_gpu_write);

        let res_gpu_r = resources_for_factory.clone();
        let p_name_r = plugin_name.clone();
        let mut veld_gpu_read = Function::new("veld_gpu_read", [ValType::I64, ValType::I64, ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let res_id = inputs[0].i64().unwrap() as u64;
                let offset = inputs[1].i64().unwrap() as u64;
                let ptr = inputs[2].i64().unwrap() as u64;
                let size = inputs[3].i64().unwrap() as u64;
                // eprintln!("[ABI:{}] GPU Read: id={}, size={}", p_name_r, res_id, size);
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
        let p_name_info = plugin_name.clone();
        let mut veld_get_info = Function::new("veld_get_info", [ValType::I64, ValType::I64], [ValType::I64], UserData::new(config_clone),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], outputs: &mut [Val], user_data: UserData<std::collections::HashMap<String, serde_json::Value>>| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, len) };
                let key_bytes = plugin.memory_bytes(handle)?;
                let key = std::str::from_utf8(key_bytes).map_err(|_| extism::Error::msg("Invalid UTF-8 key"))?;
                
                // eprintln!("[ABI:{}] Get Info: key='{}'", p_name_info, key);
                
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

        let p_name_in_len = plugin_name.clone();
        let mut v_input_len = Function::new("veld_input_len", [], [ValType::I64], UserData::new(()),
            move |plugin: &mut CurrentPlugin, _inputs: &[Val], outputs: &mut [Val], _| {
                let ctx: CallContext = plugin.host_context::<CallContext>()?.clone();
                let inner = ctx.0.lock().unwrap();
                // eprintln!("[ABI:{}] Input Len: {}", p_name_in_len, inner.input.len());
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

        let p_name_out = plugin_name.clone();
        let mut v_output_set = Function::new("veld_output_set", [ValType::I64, ValType::I64], [], UserData::new(()),
            move |plugin: &mut CurrentPlugin, inputs: &[Val], _outputs: &mut [Val], _| {
                let ptr = inputs[0].i64().unwrap() as u64;
                let len = inputs[1].i64().unwrap() as u64;
                let handle = unsafe { MemoryHandle::new(ptr, len) };
                let data = plugin.memory_bytes(handle)?.to_vec();
                // eprintln!("[ABI:{}] Output Set: {} bytes", p_name_out, data.len());
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

    let d_clone = dispatcher.clone();
    let is_visible_clone = is_visible.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let node = Arc::new(VeldmapNode::new(endpoint, d_clone.clone()).await.unwrap());
        tokio::spawn(async move { let _ = node.run().await; });
        log::info!("Core ready. Heartbeat started...");
        
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;
            if is_visible_clone.load(Ordering::Relaxed) {
                if let Err(e) = d_clone.call("data-browser", "render", vec![]).await {
                    log::error!("Render call failed: {}", e);
                }
            }
        }
    });

    event_loop.run(move |event: Event<()>, window_target: &EventLoopWindowTarget<()>| {
        window_target.set_control_flow(ControlFlow::Wait);

        match event {
            Event::UserEvent(()) => {
                let mut last_draw_cmd = None;
                while let Ok(cmd) = rx.try_recv() { last_draw_cmd = Some(cmd); }

                if let Some(AppCommand::Draw(id, w, h)) = last_draw_cmd {
                    let size = window.inner_size();
                    if is_visible.load(Ordering::SeqCst) && size.width > 0 && size.height > 0 && w > 0 && h > 0 {
                        if Some(id) != app_texture_id || (w, h) != last_size || app_bind_group.is_none() {
                            if let Some(veldmap_host_core::resources::Resource::Texture { texture, .. }) = resources.get_resource(id) {
                                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let device = resources.get_device();
                                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { 
                                    label: None, 
                                    layout: &bind_group_layout, 
                                    entries: &[
                                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, 
                                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }
                                    ] 
                                });
                                app_texture_id = Some(id);
                                app_bind_group = Some(bind_group);
                                last_size = (w, h);
                            }
                        }
                        window.request_redraw();
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let size = window.inner_size();
                if is_visible.load(Ordering::SeqCst) && size.width > 0 && size.height > 0 && config.width > 0 && config.height > 0 {
                    if let Some(bind_group) = &app_bind_group {
                        match surface.get_current_texture() {
                            Ok(frame) => {
                                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let mut encoder = resources.get_device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                                {
                                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { 
                                        label: None, 
                                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { 
                                            view: &view, 
                                            resolve_target: None, 
                                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } 
                                        })], 
                                        depth_stencil_attachment: None, ..Default::default() 
                                    });
                                    rp.set_pipeline(&render_pipeline);
                                    rp.set_bind_group(0, bind_group, &[]);
                                    rp.draw(0..3, 0..1);
                                }
                                resources.get_device().poll(wgpu::Maintain::Wait);
                                queue_arc.submit(Some(encoder.finish()));
                                frame.present();
                            }
                            Err(wgpu::SurfaceError::Outdated) => {}
                            Err(e) => log::error!("Surface error: {:?}", e),
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                if size.width > 0 && size.height > 0 {
                    is_visible.store(true, Ordering::SeqCst);
                    config.width = size.width; config.height = size.height;
                    surface.configure(&device_arc, &config);
                    window.request_redraw();
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Resize(veldmap_host_core::ui::ResizeEvent { width: size.width, height: size.height, scale_factor: window.scale_factor() as f32 })) };
                    let d_clone = dispatcher.clone();
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; });
                } else {
                    is_visible.store(false, Ordering::SeqCst);
                }
            }
            Event::WindowEvent { event: WindowEvent::Occluded(occluded), .. } => {
                is_visible.store(!occluded, Ordering::SeqCst);
                if !occluded { window.request_redraw(); }
            }
            Event::WindowEvent { event: WindowEvent::Focused(focused), .. } => {
                if focused && is_visible.load(Ordering::SeqCst) { window.request_redraw(); }
            }
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                let pressed = state == winit::event::ElementState::Pressed;
                let btn = match button { winit::event::MouseButton::Left => 1, winit::event::MouseButton::Right => 2, winit::event::MouseButton::Middle => 3, _ => 0 };
                let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Click(veldmap_host_core::ui::ClickEvent { x: cursor_pos.0, y: cursor_pos.1, button: btn, pressed })) };
                let d_clone = dispatcher.clone();
                let busy_clone = ui_busy.clone();
                tokio::spawn(async move { busy_clone.store(true, Ordering::SeqCst); let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                cursor_pos = (position.x as f32, position.y as f32);
                if !ui_busy.load(Ordering::SeqCst) && last_cursor_sent_time.elapsed() >= std::time::Duration::from_millis(50) {
                    let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::CursorMoved(veldmap_host_core::ui::CursorMovedEvent { x: cursor_pos.0, y: cursor_pos.1 })) };
                    let d_clone = dispatcher.clone();
                    let busy_clone = ui_busy.clone();
                    busy_clone.store(true, Ordering::SeqCst);
                    tokio::spawn(async move { let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
                    last_cursor_sent_time = std::time::Instant::now();
                }
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 20.0, y * 20.0),
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };
                let ev = veldmap_host_core::ui::UiEvent { event: Some(veldmap_host_core::ui::ui_event::Event::Scroll(veldmap_host_core::ui::ScrollEvent { delta_x: dx, delta_y: dy })) };
                let d_clone = dispatcher.clone();
                let busy_clone = ui_busy.clone();
                tokio::spawn(async move { busy_clone.store(true, Ordering::SeqCst); let _ = d_clone.call("data-browser", "handle_ui_event", ev.encode_to_vec()).await; busy_clone.store(false, Ordering::SeqCst); });
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => { window_target.exit(); }
            _ => (),
        }
    })?;

    Ok(())
}
