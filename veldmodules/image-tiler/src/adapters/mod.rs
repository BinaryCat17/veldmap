//! Адаптеры форматов: описать растр по заголовку и произвести тайлы.
//!
//! Формат определяется по содержимому — расширение врёт чаще заголовка, — и
//! байты идут окнами из ресурса: файл это, сеть или память, отсюда не видно.
//! Разница между форматами одна: умеет ли он отдать произвольный тайл
//! дёшево. Тайловый TIFF с обзорами — умеет (produce читает ровно нужные
//! чанки); всё остальное — последовательный проход, который строит каскадом
//! все уровни разом (см. cascade.rs), и запрошенные тайлы уезжают по мере
//! прохода, сверху вниз.

use std::cell::Cell;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::rc::Rc;

use image::ImageFormat;

use super::cascade::Emit;

pub mod full;
pub mod jp2;
pub mod jpeg;
pub mod png;
pub mod tiff;

/// Потолок стороны источника. Полосы каскада растут от ширины
/// (≈ width × 8 КБ на все уровни), и лимит линейной памяти инстанса в 1 ГБ
/// требует назвать границу заранее — честным отказом, а не смертью в
/// середине прохода.
pub const MAX_SOURCE_SIDE: u32 = 65_536;

/// Потолок для путей без потоковой развёртки (gif/bmp/webp, Adam7-PNG,
/// декодированный кадр JPEG): такой кадр живёт в памяти целиком.
pub const FULL_DECODE_BUDGET: u64 = 256 * 1024 * 1024;

/// Что известно о растре без декодирования.
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
}

pub enum Kind {
    Png,
    Jpeg,
    /// JPEG 2000: декодируется целиком, но сразу в масштаб уровня (пропуск
    /// уровней DWT); бюджет памяти проверяет сам адаптер.
    Jp2,
    Tiff(tiff::Layout),
    /// Форматы без потокового пути: декодируются целиком, они малы по природе.
    Full(ImageFormat),
}

impl Info {
    /// Произвольный тайл дёшев: не нужен полный проход по файлу.
    pub fn random_access(&self) -> bool {
        matches!(&self.kind, Kind::Tiff(layout) if layout.random_access())
    }
}

/// Заголовок растра: формат, размеры, раскладка. Дешёвый и для гигабайтного
/// файла — читаются заголовки и каталоги, не пиксели.
pub fn describe(resource_id: u64, len: u64, bytes: &Rc<Cell<u64>>) -> Result<Info, String> {
    let mut reader = Metered::new(resource_id, len, bytes.clone());
    let mut head = [0u8; 32];
    let read = reader.read(&mut head).map_err(|e| format!("чтение заголовка: {}", e))?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    // JPEG 2000 крейт `image` не знает — его сигнатуры смотрятся до него.
    if head.starts_with(jp2::JP2_MAGIC) || head.starts_with(jp2::CODESTREAM_MAGIC) {
        let info = jp2::describe(reader, len)?;
        return checked(info);
    }

    // В отказе перечислено, что вообще открывается: файл без сигнатуры
    // изображения — обычное дело в каталоге (сырец L0, разметка, XML), и
    // «не распознан» без списка читается как поломка, а не как ответ.
    let format = image::guess_format(&head[..read]).map_err(|_| {
        "по заголовку это не изображение (открываются PNG, JPEG, TIFF, JPEG 2000, GIF, BMP, WebP)"
            .to_string()
    })?;

    let info = match format {
        ImageFormat::Png => png::describe(reader),
        ImageFormat::Jpeg => jpeg::describe(reader),
        ImageFormat::Tiff => tiff::describe(reader),
        // Набор обязан совпадать с features крейта `image` в config.yaml —
        // фича без рукава молча даст «не поддерживается».
        ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP => full::describe(reader, format),
        other => Err(format!("формат {:?} не поддерживается", other)),
    }?;
    checked(info)
}

/// Общие пределы описанного растра — какой бы адаптер его ни описал.
fn checked(info: Info) -> Result<Info, String> {
    if info.width == 0 || info.height == 0 {
        return Err("пустой растр".to_string());
    }
    if info.width > MAX_SOURCE_SIDE || info.height > MAX_SOURCE_SIDE {
        return Err(format!(
            "{}×{}: сторона больше потолка {} — полосы прохода не влезут в память инстанса",
            info.width, info.height, MAX_SOURCE_SIDE
        ));
    }
    Ok(info)
}

/// Произвести тайлы уровня `level`. `wants` использует только путь с
/// произвольным доступом — он читает ровно запрошенное; последовательный
/// проход отдаёт в `emit` все тайлы всех уровней, а кто из них кому нужен,
/// решает приёмник.
pub fn produce(
    resource_id: u64,
    len: u64,
    info: &Info,
    level: u32,
    wants: &[(u32, u32)],
    bytes: &Rc<Cell<u64>>,
    emit: Emit,
) -> Result<(), String> {
    let reader = Metered::new(resource_id, len, bytes.clone());
    match &info.kind {
        Kind::Tiff(layout) if layout.random_access() => {
            tiff::produce_direct(reader, info, layout, level, wants, emit)
        }
        Kind::Tiff(layout) => tiff::produce_pass(reader, info, layout, emit),
        Kind::Png => png::produce_pass(reader, info, emit),
        Kind::Jpeg => jpeg::produce(reader, info, level, emit),
        Kind::Jp2 => jp2::produce(reader, len, info, level, emit),
        Kind::Full(format) => full::produce(reader, info, *format, emit),
    }
}

/// Развёртка сэмплов в RGBA8 — общая всем адаптерам: 1 канал — серый,
/// 2 — серый с альфой, 3 — RGB, 4 — как есть.
pub fn to_rgba(samples: &[u8], channels: usize, pixels: usize) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in samples.chunks_exact(channels).take(pixels) {
        let (rgb, alpha) = match channels {
            1 => ([px[0], px[0], px[0]], 255),
            2 => ([px[0], px[0], px[0]], px[1]),
            3 => ([px[0], px[1], px[2]], 255),
            _ => ([px[0], px[1], px[2]], px[3]),
        };
        rgba.extend_from_slice(&rgb);
        rgba.push(alpha);
    }
    rgba
}

/// Читатель ресурса со счётчиком прочитанного — из него растёт честный
/// прогресс долгого прохода. Счётчик — дальняя достигнутая позиция, а не
/// сумма чтений: повторные окна после seek назад не приближают конец файла
/// и прогрессом не являются, а сумма перевалила бы за размер. Разделён с
/// приёмником тайлов через Rc — инстанс однопоточный.
pub struct Metered {
    inner: veldsdk::ResourceReader,
    /// Текущая позиция: и read, и consume двигают её сами, потому что
    /// спрашивать её у ресурса — лишний ABI-вызов на каждое чтение.
    pos: u64,
    consumed: Rc<Cell<u64>>,
}

impl Metered {
    pub fn new(resource_id: u64, len: u64, consumed: Rc<Cell<u64>>) -> Self {
        Self { inner: veldsdk::ResourceReader::new(resource_id, len), pos: 0, consumed }
    }

    fn advance(&mut self, n: usize) {
        self.pos += n as u64;
        if self.pos > self.consumed.get() {
            self.consumed.set(self.pos);
        }
    }
}

impl Read for Metered {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.advance(n);
        Ok(n)
    }
}

impl BufRead for Metered {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.advance(amt);
        self.inner.consume(amt);
    }
}

impl Seek for Metered {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let at = self.inner.seek(pos)?;
        self.pos = at;
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_rgba_expands_all_channel_counts() {
        assert_eq!(to_rgba(&[7], 1, 1), vec![7, 7, 7, 255]);
        assert_eq!(to_rgba(&[7, 9], 2, 1), vec![7, 7, 7, 9]);
        assert_eq!(to_rgba(&[1, 2, 3], 3, 1), vec![1, 2, 3, 255]);
        assert_eq!(to_rgba(&[1, 2, 3, 4], 4, 1), vec![1, 2, 3, 4]);
        // Хвост за пределами pixels отбрасывается — у краевых чанков TIFF
        // данные бывают шире полезной части.
        assert_eq!(to_rgba(&[1, 2, 3, 4, 5, 6], 3, 1), vec![1, 2, 3, 255]);
    }
}
