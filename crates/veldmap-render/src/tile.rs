use std::sync::Arc;

use crate::RenderMetrics;

pub const TILE_ID: &str = "task2-rgba-8x8-v1";
pub const TILE_DIMENSION: u32 = 8;
pub const TILE_BYTES: u64 = TILE_DIMENSION as u64 * TILE_DIMENSION as u64 * 4;

pub struct GpuTile {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl GpuTile {
    pub fn upload_deterministic(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        metrics: &Arc<RenderMetrics>,
    ) -> Arc<Self> {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(TILE_ID),
            size: wgpu::Extent3d {
                width: TILE_DIMENSION,
                height: TILE_DIMENSION,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let pixels = deterministic_pixels();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE_DIMENSION * 4),
                rows_per_image: Some(TILE_DIMENSION),
            },
            texture.size(),
        );
        drop(pixels);

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shared tile"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        metrics.record_upload(TILE_BYTES);
        Arc::new(Self {
            _texture: texture,
            bind_group,
        })
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

fn deterministic_pixels() -> Vec<u8> {
    const COLORS: [[u8; 4]; 4] = [
        [239, 68, 68, 255],
        [34, 197, 94, 255],
        [59, 130, 246, 255],
        [250, 204, 21, 255],
    ];

    let mut pixels = Vec::with_capacity(TILE_BYTES as usize);
    for y in 0..TILE_DIMENSION {
        for x in 0..TILE_DIMENSION {
            let quadrant =
                usize::from(x >= TILE_DIMENSION / 2) + 2 * usize::from(y >= TILE_DIMENSION / 2);
            pixels.extend_from_slice(&COLORS[quadrant]);
        }
    }
    pixels
}
