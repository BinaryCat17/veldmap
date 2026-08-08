//! Перевод форматов текстур между протоколом и wgpu.
//!
//! Отдельный модуль, потому что нужен обоим: memory считает по формату длину
//! строки при заливке пикселей, graphics — создаёт им пайплайны. Лежи он в
//! одном из них, второй импортировал бы первый ради трёх функций, и
//! memory с graphics ссылались бы друг на друга.

use veldmap_host_bindings::proto::graphics::TextureFormat;

pub fn bytes_per_pixel(format_proto: i32) -> u32 {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR8Unorm => 1,
        TextureFormat::TexR32Float => 4,
        TextureFormat::TexRgba16Float => 8,
        TextureFormat::TexRgba32Float => 16,
        _ => 4,
    }
}

pub fn proto_to_wgpu(format_proto: i32) -> wgpu::TextureFormat {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
        TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
        TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
        TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        _ => wgpu::TextureFormat::Rgba8Unorm,
    }
}

pub fn wgpu_to_proto(fmt: wgpu::TextureFormat) -> i32 {
    match fmt {
        wgpu::TextureFormat::R32Float => TextureFormat::TexR32Float as i32,
        wgpu::TextureFormat::Rgba16Float => TextureFormat::TexRgba16Float as i32,
        wgpu::TextureFormat::Rgba32Float => TextureFormat::TexRgba32Float as i32,
        wgpu::TextureFormat::R8Unorm => TextureFormat::TexR8Unorm as i32,
        wgpu::TextureFormat::Bgra8UnormSrgb => TextureFormat::TexBgra8UnormSrgb as i32,
        wgpu::TextureFormat::Rgba8UnormSrgb => TextureFormat::TexRgba8UnormSrgb as i32,
        _ => TextureFormat::TexRgba8Unorm as i32,
    }
}
