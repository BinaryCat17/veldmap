use crate::camera::{OrbitCamera, CameraController};
use crate::tiling::TileManager;
use crate::{RenderConfig, RenderBackend};
use veldmap_core::data_module::{TileId, DemTile};
use veldmap_core::render_module::Renderer;
use glam::{DMat4, Mat4};
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};

const MAX_DEM_TILES: usize = 64;
const TILE_SIZE: u32 = 256;

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_inv: [[f32; 4]; 4],
    proj_inv: [[f32; 4]; 4],
    position: [f32; 3],
    padding: f32,
}

pub struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: Mutex<wgpu::SurfaceConfiguration>,
    pub width: Mutex<u32>,
    pub height: Mutex<u32>,
    render_pipeline: wgpu::RenderPipeline,
    camera_uniform: Mutex<CameraUniform>,
    camera_buffer: wgpu::Buffer,
    camera_bind_group_layout: wgpu::BindGroupLayout,
    camera_bind_group: Mutex<wgpu::BindGroup>,
    
    // DEM data is now stored in a storage buffer to avoid Vulkan partial copy limitations on some drivers
    dem_storage_buffer: wgpu::Buffer,
    dem_sampler: wgpu::Sampler,
    
    geoid_texture: Mutex<wgpu::Texture>,
    geoid_view: Mutex<wgpu::TextureView>,
    
    indirection_texture: wgpu::Texture,
    indir_view: wgpu::TextureView,
    
    pub tile_manager: Mutex<TileManager>,

    pub camera: Mutex<OrbitCamera>,
    pub camera_controller: CameraController,
}

impl Renderer for State {
    fn render(&self) -> Result<(), String> {
        let (width, height) = {
            let w = self.width.lock().unwrap();
            let h = self.height.lock().unwrap();
            (*w, *h)
        };
        if width == 0 || height == 0 { return Ok(()); }

        let output = self.surface.get_current_texture().map_err(|e| format!("{:?}", e))?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Ray Marching Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, ..Default::default()
            });
            rp.set_pipeline(&self.render_pipeline);
            let bg = self.camera_bind_group.lock().unwrap();
            rp.set_bind_group(0, &*bg, &[]);
            rp.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    fn resize(&self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            {
                let mut w = self.width.lock().unwrap();
                let mut h = self.height.lock().unwrap();
                let mut config = self.config.lock().unwrap();
                
                *w = width;
                *h = height;
                config.width = width;
                config.height = height;
                self.surface.configure(&self.device, &config);
            }
            self.update();
        }
    }

    fn update(&self) {
        let (eye, view_inv, proj_inv) = {
            let camera = self.camera.lock().unwrap();
            let width = *self.width.lock().unwrap();
            let height = *self.height.lock().unwrap();

            let eye = camera.get_position();
            let rotation_inv = DMat4::from_quat(camera.orientation.conjugate());
            let translation_inv = DMat4::from_translation(-eye);
            let view = (rotation_inv * translation_inv).as_mat4();
            let view_inv = view.inverse();
            let aspect = if height > 0 { width as f32 / height as f32 } else { 1.0 };
            let proj = Mat4::perspective_rh(45.0f32.to_radians(), aspect, 0.1, 1000.0);
            let proj_inv = proj.inverse();
            (eye, view_inv, proj_inv)
        };

        {
            let mut uniform = self.camera_uniform.lock().unwrap();
            uniform.view_inv = view_inv.to_cols_array_2d();
            uniform.proj_inv = proj_inv.to_cols_array_2d();
            uniform.position = [eye.x as f32, eye.y as f32, eye.z as f32];
            self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&[*uniform]));
        }
        
        {
            let mut tm = self.tile_manager.lock().unwrap();
            tm.update_indirection();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.indirection_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &tm.indirection_data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(128), rows_per_image: Some(64) },
                wgpu::Extent3d { width: 128, height: 64, depth_or_array_layers: 1 }
            );
        }
    }

    fn upload_tile(&self, id: TileId, dem: Arc<DemTile>) {
        let mut tm = self.tile_manager.lock().unwrap();
        if let Some(slot) = tm.assign_slot(id) {
            let offset = slot as u64 * (TILE_SIZE as u64 * TILE_SIZE as u64 * 4);
            self.queue.write_buffer(
                &self.dem_storage_buffer,
                offset,
                bytemuck::cast_slice(&dem.heights),
            );
        }
    }

    fn set_geoid(&self, dem: Arc<DemTile>) {
        let size = wgpu::Extent3d { width: dem.width as u32, height: dem.height as u32, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("EGM2008 Geoid"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&dem.heights),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * dem.width as u32), rows_per_image: Some(dem.height as u32) },
            size,
        );
        
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        
        {
            let mut g_tex = self.geoid_texture.lock().unwrap();
            let mut g_view = self.geoid_view.lock().unwrap();
            *g_tex = texture;
            *g_view = view;
            
            let mut bg = self.camera_bind_group.lock().unwrap();
            *bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.camera_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.camera_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.dem_storage_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.dem_sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&*g_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.indir_view) },
                ],
                label: Some("camera_bind_group_updated"),
            });
        }
    }

    fn camera_zoom(&self, delta: f64) {
        let mut camera = self.camera.lock().unwrap();
        self.camera_controller.process_mouse_scroll(delta, &mut camera);
    }

    fn camera_move(&self, dx: f64, dy: f64) {
        let mut camera = self.camera.lock().unwrap();
        self.camera_controller.process_mouse_motion(dx, dy, &mut camera);
    }
}

impl State {
    pub async fn new<W>(window: W, config: RenderConfig) -> Self
    where
        W: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle + Send + Sync + 'static,
    {
        let backends = match config.backend {
            RenderBackend::Vulkan => wgpu::Backends::VULKAN,
            RenderBackend::Metal => wgpu::Backends::METAL,
            RenderBackend::Dx12 => wgpu::Backends::DX12,
            RenderBackend::Gl => wgpu::Backends::GL,
            RenderBackend::BrowserWebGpu => wgpu::Backends::BROWSER_WEBGPU,
        };

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(window).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find graphics adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surf_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: 800,
            height: 600,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surf_config);

        let dem_storage_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DEM Storage Buffer"),
            size: (MAX_DEM_TILES * TILE_SIZE as usize * TILE_SIZE as usize * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let indirection_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Indirection Texture"),
            size: wgpu::Extent3d { width: 128, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let indir_view = indirection_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let geoid_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Geoid Texture Placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let geoid_view = geoid_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let dem_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
            label: Some("camera_bind_group_layout"),
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[CameraUniform { view_inv: Mat4::IDENTITY.to_cols_array_2d(), proj_inv: Mat4::IDENTITY.to_cols_array_2d(), position: [0.0; 3], padding: 0.0 }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: dem_storage_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&dem_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&geoid_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&indir_view) },
            ],
            label: Some("camera_bind_group"),
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Layout"), bind_group_layouts: &[&camera_bind_group_layout], push_constant_ranges: &[] });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RayMarching Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), targets: &[Some(wgpu::ColorTargetState { format: surf_config.format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
        });

        Self {
            surface, device, queue, config: Mutex::new(surf_config), width: Mutex::new(800), height: Mutex::new(600), render_pipeline,
            camera_uniform: Mutex::new(CameraUniform { view_inv: Mat4::IDENTITY.to_cols_array_2d(), proj_inv: Mat4::IDENTITY.to_cols_array_2d(), position: [0.0; 3], padding: 0.0 }),
            camera_buffer, 
            camera_bind_group_layout,
            camera_bind_group: Mutex::new(camera_bind_group),
            dem_storage_buffer,
            dem_sampler,
            geoid_texture: Mutex::new(geoid_texture),
            geoid_view: Mutex::new(geoid_view),
            indirection_texture, 
            indir_view,
            tile_manager: Mutex::new(TileManager::new(MAX_DEM_TILES)),
            camera: Mutex::new(OrbitCamera::new(12_000_000.0, 0.0, 0.0)),
            camera_controller: CameraController::new(0.003),
        }
    }
}