//! GPU Compositor for VeldMap Host
//! 
//! Handles final composition of UI and plugin renders to the screen.
//! Uses a simple fullscreen triangle blit for UI texture overlay.

use veldmap_host_core::resources::ResourceManager;

pub struct Compositor {
    blit_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Compositor BGL"),
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
            label: Some("Compositor Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Compositor Blit Pipeline"),
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Compositor Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            blit_pipeline,
            bind_group_layout,
            sampler,
        }
    }

    /// Create a bind group for blitting a texture
    pub fn create_bind_group(&self, device: &wgpu::Device, texture_view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Compositor Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Render the UI texture to the surface via blit
    pub fn blit_ui(&self, render_pass: &mut wgpu::RenderPass, bind_group: &wgpu::BindGroup) {
        render_pass.set_pipeline(&self.blit_pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// Render commands to their target surfaces
pub fn execute_offscreen_passes(
    encoder: &mut wgpu::CommandEncoder,
    render_queue: &std::sync::Mutex<Vec<veldmap_host_core::compute::Submit>>,
    resources: &std::sync::Arc<ResourceManager>,
) -> Vec<veldmap_host_core::compute::Submit> {
    let mut surface_cmds = Vec::new();
    
    let queue = {
        let mut q = render_queue.lock().unwrap();
        std::mem::take(&mut *q)
    };
    
    for req in queue {
        if req.target_texture_view_id == 0 {
            // Target is surface - defer to surface pass
            surface_cmds.push(req);
        } else {
            // Target is offscreen texture - render now
            if let Some(veldmap_host_core::resources::Resource::TextureView(target_view)) = 
                resources.get_resource(req.target_texture_view_id, req.instance_id) 
            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Offscreen Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });

                if let Some(ref cb) = req.command_buffer {
                    let _ = veldmap_host_compute::execute_render_commands(
                        &mut rp, cb, resources, 2048, 2048, req.instance_id
                    );
                }
            }
        }
    }
    
    surface_cmds
}
