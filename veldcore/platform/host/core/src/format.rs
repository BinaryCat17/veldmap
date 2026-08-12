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
///
/// Match без запасного рукава — здесь и ниже: формат, добавленный в протокол,
/// обязан получить свою строку в этих таблицах, иначе он молча считался бы
/// четырёхбайтным RGBA. С полным перечислением забытый вариант — ошибка
/// компиляции хоста, а не тихая деградация.
pub fn bytes_per_pixel(format_proto: i32) -> u32 {
    match TextureFormat::try_from(format_proto).unwrap_or(TextureFormat::TexRgba8Unorm) {
        TextureFormat::TexR8Unorm => 1,
        TextureFormat::TexR32Float => 4,
        TextureFormat::TexRgba16Float => 8,
        TextureFormat::TexRgba32Float => 16,
        TextureFormat::TexRgba8Unorm
        | TextureFormat::TexRgba8UnormSrgb
        | TextureFormat::TexBgra8UnormSrgb => 4,
        // Недостижимо: заливку в глубину отсекает upload_image (см. is_depth).
        TextureFormat::TexDepth32Float => 4,
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
        TextureFormat::TexRgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        TextureFormat::TexR32Float => wgpu::TextureFormat::R32Float,
        TextureFormat::TexRgba16Float => wgpu::TextureFormat::Rgba16Float,
        TextureFormat::TexRgba32Float => wgpu::TextureFormat::Rgba32Float,
        TextureFormat::TexR8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureFormat::TexBgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        TextureFormat::TexRgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureFormat::TexDepth32Float => wgpu::TextureFormat::Depth32Float,
    }
}

/// Обратный ход. Запасной рукав здесь неустраним: слева весь wgpu-энум, а не
/// наш протокол, — но каждый формат, названный в proto_to_wgpu, обязан иметь
/// здесь свою строку.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Все форматы протокола: перечислением, чтобы новый вариант, добавленный
    /// в enum, не остался за бортом теста.
    const ALL: [TextureFormat; 8] = [
        TextureFormat::TexRgba8Unorm,
        TextureFormat::TexRgba8UnormSrgb,
        TextureFormat::TexBgra8UnormSrgb,
        TextureFormat::TexR8Unorm,
        TextureFormat::TexR32Float,
        TextureFormat::TexRgba16Float,
        TextureFormat::TexRgba32Float,
        TextureFormat::TexDepth32Float,
    ];

    #[test]
    fn proto_wgpu_roundtrip() {
        for format in ALL {
            assert_eq!(
                wgpu_to_proto(proto_to_wgpu(format as i32)),
                format as i32,
                "{:?} должен пережить перевод туда и обратно",
                format
            );
        }
    }

    #[test]
    fn depth_is_exactly_one_format() {
        for format in ALL {
            assert_eq!(
                is_depth(format as i32),
                format == TextureFormat::TexDepth32Float,
                "{:?}", format
            );
        }
    }

    #[test]
    fn pixel_lengths() {
        assert_eq!(bytes_per_pixel(TextureFormat::TexR8Unorm as i32), 1);
        assert_eq!(bytes_per_pixel(TextureFormat::TexRgba8Unorm as i32), 4);
        assert_eq!(bytes_per_pixel(TextureFormat::TexRgba16Float as i32), 8);
        assert_eq!(bytes_per_pixel(TextureFormat::TexRgba32Float as i32), 16);
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
