use std::sync::{Arc, Mutex};

use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use wgpu::util::DeviceExt as _;

use crate::GpuTile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportKind {
    Viewer,
    Globe,
}

impl ViewportKind {
    pub const ALL: [Self; 2] = [Self::Viewer, Self::Globe];

    pub fn label(self) -> &'static str {
        match self {
            Self::Viewer => "Viewer",
            Self::Globe => "Globe",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Viewer => 0,
            Self::Globe => 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ViewportTransform {
    pub pan: [f32; 2],
    pub scale: f32,
    pub rotation: f32,
}

impl Default for ViewportTransform {
    fn default() -> Self {
        Self {
            pan: [0.0, 0.0],
            scale: 0.9,
            rotation: 0.0,
        }
    }
}

impl ViewportTransform {
    fn to_bytes(self) -> [u8; 16] {
        let mut bytes = [0; 16];
        for (index, value) in [self.pan[0], self.pan[1], self.scale, self.rotation]
            .into_iter()
            .enumerate()
        {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

#[derive(Clone, Debug, Default)]
pub struct ViewportSnapshot {
    pub draws: u64,
    pub tile_handle: u64,
    pub viewport_px: [i32; 2],
    pub clip_px: [i32; 2],
    pub screen_px: [u32; 2],
    pub pixels_per_point: f32,
    pub clip_modes: u8,
    pub size_changes: u32,
}

#[derive(Clone, Debug, Default)]
pub struct MetricsSnapshot {
    pub tile_uploads: u64,
    pub tile_upload_bytes: u64,
    pub uniform_uploads: u64,
    pub uniform_upload_bytes: u64,
    pub surface_reconfigurations: u32,
    pub viewports: [ViewportSnapshot; 2],
}

#[derive(Default)]
pub struct RenderMetrics {
    snapshot: Mutex<MetricsSnapshot>,
}

impl RenderMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot.lock().expect("render metrics lock").clone()
    }

    pub fn record_upload(&self, bytes: u64) {
        let mut snapshot = self.snapshot.lock().expect("render metrics lock");
        snapshot.tile_uploads += 1;
        snapshot.tile_upload_bytes += bytes;
    }

    fn record_prepare(&self) {
        let mut snapshot = self.snapshot.lock().expect("render metrics lock");
        snapshot.uniform_uploads += 1;
        snapshot.uniform_upload_bytes += 16;
    }

    fn record_paint(&self, kind: ViewportKind, tile_handle: u64, info: egui::PaintCallbackInfo) {
        let viewport = info.viewport_in_pixels();
        let clip = info.clip_rect_in_pixels();
        let viewport_px = [viewport.width_px, viewport.height_px];
        let clipped = clip.width_px < viewport.width_px || clip.height_px < viewport.height_px;
        let mut snapshot = self.snapshot.lock().expect("render metrics lock");
        let previous_screen = snapshot.viewports[kind.index()].screen_px;
        if previous_screen != [0, 0] && previous_screen != info.screen_size_px {
            snapshot.surface_reconfigurations += 1;
        }
        let viewport_snapshot = &mut snapshot.viewports[kind.index()];
        viewport_snapshot.draws += 1;
        viewport_snapshot.tile_handle = tile_handle;
        if viewport_snapshot.viewport_px != [0, 0] && viewport_snapshot.viewport_px != viewport_px {
            viewport_snapshot.size_changes += 1;
        }
        viewport_snapshot.viewport_px = viewport_px;
        viewport_snapshot.clip_px = [clip.width_px, clip.height_px];
        viewport_snapshot.screen_px = info.screen_size_px;
        viewport_snapshot.pixels_per_point = info.pixels_per_point;
        viewport_snapshot.clip_modes |= if clipped { 1 } else { 2 };
    }
}

pub struct SharedRenderResources {
    pipeline: wgpu::RenderPipeline,
    tile_layout: wgpu::BindGroupLayout,
    tile_sampler: wgpu::Sampler,
    geometries: [Geometry; 2],
}

impl SharedRenderResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shared tile layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let tile_sampler_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shared sampler layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                }],
            });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport transform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shared tile pipeline layout"),
            bind_group_layouts: &[
                Some(&tile_layout),
                Some(&tile_sampler_layout),
                Some(&viewport_layout),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared tile shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shared_tile.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shared tile pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let tile_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shared tile sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let sampler_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shared sampler bind group"),
            layout: &tile_sampler_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&tile_sampler),
            }],
        });
        let geometries = ViewportKind::ALL
            .map(|kind| Geometry::new(device, kind, &viewport_layout, &sampler_bind_group));
        Self {
            pipeline,
            tile_layout,
            tile_sampler,
            geometries,
        }
    }

    pub fn tile_layout(&self) -> &wgpu::BindGroupLayout {
        &self.tile_layout
    }
}

struct Geometry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    transform_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    sampler_bind_group: wgpu::BindGroup,
}

impl Geometry {
    fn new(
        device: &wgpu::Device,
        kind: ViewportKind,
        viewport_layout: &wgpu::BindGroupLayout,
        sampler_bind_group: &wgpu::BindGroup,
    ) -> Self {
        let (vertices, indices) = geometry(kind);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewport vertices"),
            contents: &vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewport indices"),
            contents: &indices,
            usage: wgpu::BufferUsages::INDEX,
        });
        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewport transform"),
            contents: &ViewportTransform::default().to_bytes(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport transform bind group"),
            layout: viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: transform_buffer.as_entire_binding(),
            }],
        });
        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32 / 2,
            transform_buffer,
            viewport_bind_group,
            sampler_bind_group: sampler_bind_group.clone(),
        }
    }
}

pub struct TileCallback {
    kind: ViewportKind,
    tile: Arc<GpuTile>,
    transform: ViewportTransform,
    metrics: Arc<RenderMetrics>,
}

impl TileCallback {
    pub fn new(
        kind: ViewportKind,
        tile: Arc<GpuTile>,
        transform: ViewportTransform,
        metrics: Arc<RenderMetrics>,
    ) -> Self {
        Self {
            kind,
            tile,
            transform,
            metrics,
        }
    }
}

impl CallbackTrait for TileCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = callback_resources
            .get::<SharedRenderResources>()
            .expect("shared render resources");
        queue.write_buffer(
            &resources.geometries[self.kind.index()].transform_buffer,
            0,
            &self.transform.to_bytes(),
        );
        self.metrics.record_prepare();
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let resources = callback_resources
            .get::<SharedRenderResources>()
            .expect("shared render resources");
        let geometry = &resources.geometries[self.kind.index()];
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, self.tile.bind_group(), &[]);
        render_pass.set_bind_group(1, &geometry.sampler_bind_group, &[]);
        render_pass.set_bind_group(2, &geometry.viewport_bind_group, &[]);
        render_pass.set_vertex_buffer(0, geometry.vertex_buffer.slice(..));
        render_pass.set_index_buffer(geometry.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..geometry.index_count, 0, 0..1);
        self.metrics
            .record_paint(self.kind, Arc::as_ptr(&self.tile) as usize as u64, info);
        let _ = &resources.tile_sampler;
    }
}

fn geometry(kind: ViewportKind) -> (Vec<u8>, Vec<u8>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    match kind {
        ViewportKind::Viewer => {
            for (position, uv) in [
                ([-0.9, -0.8], [0.0, 0.0]),
                ([0.9, -0.8], [1.0, 0.0]),
                ([0.9, 0.8], [1.0, 1.0]),
                ([-0.9, 0.8], [0.0, 1.0]),
            ] {
                push_vertex(&mut vertices, position, uv);
            }
            for index in [0, 1, 2, 0, 2, 3] {
                push_index(&mut indices, index);
            }
        }
        ViewportKind::Globe => {
            push_vertex(&mut vertices, [0.0, 0.0], [0.5, 0.5]);
            const SEGMENTS: u16 = 64;
            for segment in 0..SEGMENTS {
                let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                let (sin, cos) = angle.sin_cos();
                let position = [sin * 0.82, cos * 0.9];
                let uv = [position[0] * 0.5 + 0.5, position[1] * 0.5 + 0.5];
                push_vertex(&mut vertices, position, uv);
            }
            for segment in 0..SEGMENTS {
                push_index(&mut indices, 0);
                push_index(&mut indices, segment + 1);
                push_index(&mut indices, (segment + 1) % SEGMENTS + 1);
            }
        }
    }
    (vertices, indices)
}

fn push_vertex(bytes: &mut Vec<u8>, position: [f32; 2], uv: [f32; 2]) {
    for value in [position[0], position[1], uv[0], uv[1]] {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
}

fn push_index(bytes: &mut Vec<u8>, index: u16) {
    bytes.extend_from_slice(&index.to_ne_bytes());
}
