//! Перевод форматов текстур между протоколом и wgpu.
//!
//! Отдельный модуль, потому что нужен обоим: memory считает по формату длину
//! строки при заливке пикселей, graphics — создаёт им пайплайны. Лежи он в
//! одном из них, второй импортировал бы первый ради трёх функций, и
//! memory с graphics ссылались бы друг на друга.

use veldmap_host_bindings::proto::graphics::TextureFormat;

/// Длина пикселя в байтах — для заливки изображения в текстуру.
///
/// У буфера глубины её нет: залить в него нечего (см. [`is_depth`]), и вызвать
/// это на нём означало бы, что заливка дошла туда, куда не должна была.
pub fn bytes_per_pixel(format_proto: i32) -> u32 {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR8Unorm => 1,
        TextureFormat::TexR32Float => 4,
        TextureFormat::TexRgba16Float => 8,
        TextureFormat::TexRgba32Float => 16,
        _ => 4,
    }
}

/// Буфер глубины отличается от цветной текстуры не только формулой пикселя:
/// его нельзя заливать, нельзя прикладывать цветным аттачментом и нельзя
/// смешивать. Один предикат на все три правила — чтобы они не разъехались.
pub fn is_depth(format_proto: i32) -> bool {
    matches!(
        TextureFormat::try_from(format_proto),
        Ok(TextureFormat::TexDepth32Float)
    )
}

pub fn proto_to_wgpu(format_proto: i32) -> wgpu::TextureFormat {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
        TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
        TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
        TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureFormat::TexDepth32Float => wgpu::TextureFormat::Depth32Float,
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
        wgpu::TextureFormat::Depth32Float => TextureFormat::TexDepth32Float as i32,
        _ => TextureFormat::TexRgba8Unorm as i32,
    }
}

/// Как сравнивать глубину. Умолчание — `Always`: неизвестное значение не должно
/// молча отбрасывать геометрию.
pub fn compare_to_wgpu(proto: i32) -> wgpu::CompareFunction {
    use veldmap_host_bindings::proto::graphics::CompareFunction as Cmp;
    match Cmp::try_from(proto).unwrap_or(Cmp::CmpAlways) {
        Cmp::CmpNever => wgpu::CompareFunction::Never,
        Cmp::CmpLess => wgpu::CompareFunction::Less,
        Cmp::CmpEqual => wgpu::CompareFunction::Equal,
        Cmp::CmpLessEqual => wgpu::CompareFunction::LessEqual,
        Cmp::CmpGreater => wgpu::CompareFunction::Greater,
        Cmp::CmpNotEqual => wgpu::CompareFunction::NotEqual,
        Cmp::CmpGreaterEqual => wgpu::CompareFunction::GreaterEqual,
        Cmp::CmpAlways => wgpu::CompareFunction::Always,
    }
}
