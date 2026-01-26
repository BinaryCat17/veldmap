use std::sync::Arc;
use std::path::Path;
use winit::{event::WindowEvent, window::Window};
use wgpu::util::DeviceExt;
use crate::mesh::{Vertex, create_sphere, create_terrain_patch};
use crate::camera::{OrbitCamera, CameraController};
use crate::dem::load_dem;
use glam::{Mat4, DMat4};

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}

struct TerrainBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    num_indices: u32,
}

pub struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    globe_vertex_buffer: wgpu::Buffer,
    globe_index_buffer: wgpu::Buffer,
    globe_num_indices: u32,
    terrain_patches: Vec<TerrainBuffers>,
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_texture: wgpu::TextureView,
    pub camera: OrbitCamera,
    pub camera_controller: CameraController,
    frame_count: u64,
}

impl State {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        
        // Оставляем ТОЛЬКО Vulkan и форсируем поддержку DZN
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            flags: wgpu::InstanceFlags::default() | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
            ..Default::default()
        });

        let surface = instance.create_surface(window).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.expect("Failed to find graphics adapter");

        println!("Selected Adapter: {:?} via {:?}", adapter.get_info().name, adapter.get_info().backend);

        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: Default::default(),
        }, None).await.unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_texture = Self::create_depth_texture(&device, &config);

        let lat = 47.0f64.to_radians();
        let lon = 39.0f64.to_radians();
        let camera = OrbitCamera::new(12_000_000.0, lon, lat);
        let camera_controller = CameraController::new(0.003);

        let camera_uniform = CameraUniform { view_proj: Mat4::IDENTITY.to_cols_array_2d() };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
            label: Some("camera_bind_group_layout"),
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() }],
            label: Some("camera_bind_group"),
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Layout"),
            bind_group_layouts: &[&camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Base"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), targets: &[Some(wgpu::ColorTargetState { format: config.format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default() }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Terrain"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), targets: &[Some(wgpu::ColorTargetState { format: config.format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: wgpu::TextureFormat::Depth32Float,
                            depth_write_enabled: true,
                            depth_compare: wgpu::CompareFunction::LessEqual,
                            stencil: wgpu::StencilState::default(),
                            bias: wgpu::DepthBiasState::default(),
                        }),
            
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let (g_vertices, g_indices) = create_sphere(256, 512);
        let globe_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&g_vertices), usage: wgpu::BufferUsages::VERTEX });
        let globe_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&g_indices), usage: wgpu::BufferUsages::INDEX });
        let globe_num_indices = g_indices.len() as u32;

        let mut terrain_patches = Vec::new();
        let dem_configs = [("data/test_tile.tif", 45.0, 38.0), ("data/rostov_tile.tif", 47.0, 39.0)];
        for (path, lat, lon) in dem_configs {
            if Path::new(path).exists() {
                if let Ok(dem) = load_dem(Path::new(path), lat, lon, 10) {
                    let (t_vertices, t_indices) = create_terrain_patch(&dem);
                    let v_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&t_vertices), usage: wgpu::BufferUsages::VERTEX });
                    let i_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&t_indices), usage: wgpu::BufferUsages::INDEX });
                    terrain_patches.push(TerrainBuffers { vertex_buffer: v_buf, index_buffer: i_buf, num_indices: t_indices.len() as u32 });
                }
            }
        }

        let mut state = Self { surface, device, queue, config, size, render_pipeline, terrain_pipeline, globe_vertex_buffer, globe_index_buffer, globe_num_indices, terrain_patches, camera_uniform, camera_buffer, camera_bind_group, depth_texture, camera, camera_controller, frame_count: 0 };
        state.update();
        state
    }

    fn create_depth_texture(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: config.width.max(1), height: config.height.max(1), depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            // Пересоздаем Surface и глубину
            self.surface.configure(&self.device, &self.config);
            self.depth_texture = Self::create_depth_texture(&self.device, &self.config);
            // Сбрасываем камеру под новый аспект
            self.update();
        }
    }

    pub fn input(&mut self, event: &WindowEvent) -> bool { self.camera_controller.process_events(event, &mut self.camera) }

    pub fn update(&mut self) {
        let eye = self.camera.get_position();
        let altitude = (self.camera.distance - 6_371_000.0).max(1.0);
        
        let near = (altitude * 0.1).clamp(10.0, 1_000_000.0);
        let far = 500_000_000.0;
        
        let safe_near = (near as f32).min(far as f32 * 0.9);

        let rotation_inv = DMat4::from_quat(self.camera.orientation.conjugate());
        let translation_inv = DMat4::from_translation(-eye);
        let view = (rotation_inv * translation_inv).as_mat4();
        
        let aspect = if self.config.height > 0 { self.config.width as f32 / self.config.height as f32 } else { 1.0 };
        let proj = Mat4::perspective_rh(45.0f32.to_radians(), aspect, safe_near, far as f32);
        
        let vp = proj * view;
        
        if !vp.is_nan() {
            self.camera_uniform.view_proj = vp.to_cols_array_2d();
            self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&[self.camera_uniform]));
        }
        
        self.frame_count += 1;
        if self.frame_count % 300 == 0 {
            self.log_memory_usage();
        }
    }

    fn log_memory_usage(&self) {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    println!("Memory Status: {}", line);
                }
            }
        }
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        // Если окно свернуто, ничего не делаем
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }

        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.002, g: 0.005, b: 0.02, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth_texture, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                ..Default::default()
            });
            rp.set_bind_group(0, &self.camera_bind_group, &[]);
            
            rp.set_pipeline(&self.render_pipeline);
            rp.set_vertex_buffer(0, self.globe_vertex_buffer.slice(..));
            rp.set_index_buffer(self.globe_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..self.globe_num_indices, 0, 0..1);
            
            rp.set_pipeline(&self.terrain_pipeline);
            for patch in &self.terrain_patches {
                rp.set_vertex_buffer(0, patch.vertex_buffer.slice(..));
                rp.set_index_buffer(patch.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..patch.num_indices, 0, 0..1);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }
}