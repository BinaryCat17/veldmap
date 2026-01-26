use bytemuck::{Pod, Zeroable};
use crate::geo::lat_lon_to_cartesian;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub lat_lon: [f32; 2],
}

impl Vertex {
    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: (std::mem::size_of::<[f32; 3]>() * 2) as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

pub fn create_sphere(lat_segments: u32, lon_segments: u32) -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for lat_idx in 0..=lat_segments {
        let theta = (lat_idx as f32 / lat_segments as f32) * 180.0 - 90.0;
        for lon_idx in 0..=lon_segments {
            let phi = (lon_idx as f32 / lon_segments as f32) * 360.0 - 180.0;
            // Сфера на 50км вглубь (гарантированное дно)
            let pos = lat_lon_to_cartesian(theta, phi, -50000.0);
            vertices.push(Vertex {
                position: pos.to_array(),
                color: [0.005, 0.01, 0.05], // Темно-темно синий
                lat_lon: [theta, phi],
            });
        }
    }

    for lat in 0..lat_segments {
        for lon in 0..lon_segments {
            let first = (lat * (lon_segments + 1) + lon) as u32;
            let second = ((lat + 1) * (lon_segments + 1) + lon) as u32;
            
            indices.push(first);
            indices.push(second);
            indices.push(first + 1);
            
            indices.push(second);
            indices.push(second + 1);
            indices.push(first + 1);
        }
    }
    (vertices, indices)
}

pub fn create_terrain_patch(dem: &crate::dem::DemData) -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let lat_step = 1.0 / (dem.height as f32 - 1.0);
    let lon_step = 1.0 / (dem.width as f32 - 1.0);

    for y in 0..dem.height {
        let lat = dem.lat_start + 1.0 - (y as f32 * lat_step);
        for x in 0..dem.width {
            let lon = dem.lon_start + (x as f32 * lon_step);
            let h = dem.heights[y * dem.width + x];
            
            // Реальный масштаб: высота в метрах без преувеличения
            // h < 0 означает глубину, и она будет отрисована ПОВЕРХ сферы-дна
            let pos = lat_lon_to_cartesian(lat, lon, h);
            
            let color = if h <= 0.0 {
                [0.1, 0.3, 0.8] // Вода
            } else if h < 30.0 {
                [0.2, 0.7, 0.2] // Зелень
            } else if h < 60.0 {
                [0.6, 0.6, 0.2] // Песок/Холмы
            } else {
                [0.8, 0.4, 0.1] // Горы
            };

            vertices.push(Vertex {
                position: pos.to_array(),
                color,
                lat_lon: [lat, lon],
            });
        }
    }

    for y in 0..dem.height - 1 {
        for x in 0..dem.width - 1 {
            let i = (y * dem.width + x) as u32;
            let next_row = i + dem.width as u32;
            
            indices.push(i);
            indices.push(i + 1);
            indices.push(next_row);

            indices.push(next_row);
            indices.push(i + 1);
            indices.push(next_row + 1);
        }
    }
    (vertices, indices)
}