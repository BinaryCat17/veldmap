//! Снятие превью с изображения: формат определяется по содержимому, пиксели
//! идут в даунсемплер по мере декодирования.
//!
//! Ключевое свойство — память не зависит от размера файла: PNG отдаётся
//! построчно, TIFF — стрипами/тайлами (и, если в файле есть уменьшенные копии,
//! читается сразу подходящая), JPEG уменьшается прямо в DCT. Байты берутся из
//! ресурса окнами (`veldsdk::ResourceReader`), то есть за ним может стоять
//! файл на диске — целиком в память он не поднимается.

use std::io::{Read, Seek, SeekFrom};

use image::ImageFormat;
use veldsdk::ResourceReader;

use crate::module::downsample::Downsampler;

/// Потолок для форматов без потокового пути (gif/bmp/webp): такие файлы малы
/// по своей природе, и полный кадр в памяти для них допустим.
const FULL_DECODE_BUDGET: u64 = 256 * 1024 * 1024;

pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// Размеры исходного изображения — превью почти всегда меньше.
    pub source: (u32, u32),
    pub rgba: Vec<u8>,
}

/// Проверка «работу пора прекращать», опрашиваемая между порциями. Декодер
/// не знает, откуда берётся отмена (см. module.rs), — ему нужен только ответ.
pub fn preview(resource_id: u64, len: u64, max_w: u32, max_h: u32) -> Result<Preview, String> {
    let mut reader = ResourceReader::new(resource_id, len);
    let mut head = [0u8; 32];
    let read = reader.read(&mut head).map_err(|e| format!("чтение заголовка: {}", e))?;
    let format = image::guess_format(&head[..read])
        .map_err(|_| "формат файла не распознан".to_string())?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    match format {
        ImageFormat::Png => png(reader, max_w, max_h),
        ImageFormat::Jpeg => jpeg(reader, max_w, max_h),
        ImageFormat::Tiff => tiff(reader, max_w, max_h),
        ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP => full(reader, format, max_w, max_h),
        other => Err(format!("формат {:?} не поддерживается", other)),
    }
}

// ── PNG: построчно ─────────────────────────────────────────────

fn png(reader: ResourceReader, max_w: u32, max_h: u32) -> Result<Preview, String> {
    let mut decoder = png::Decoder::new(reader);
    // Палитра разворачивается, 1/2/4 бита дополняются, 16 бит ужимаются до 8 —
    // ниже остаются только четыре комбинации каналов.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("png: {}", e))?;

    let (w, h, interlaced) = {
        let info = reader.info();
        (info.width, info.height, info.interlaced)
    };
    // У чересстрочного PNG строки идут в порядке Adam7, а не сверху вниз:
    // потокового пути для него нет, декодируем кадр целиком.
    if interlaced {
        // None — размер кадра не представим в usize: для 32-битного wasm это
        // достижимо на больших снимках, и отказ здесь честнее паники в vec!.
        let size = reader.output_buffer_size()
            .ok_or_else(|| "png: кадр не помещается в адресное пространство".to_string())?;
        let mut buf = vec![0u8; size];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("png: {}", e))?;
        let channels = usize::from(info.color_type.samples());
        let mut sink = Sink::new(w, h, max_w, max_h);
        sink.rows(&buf, w, h, channels);
        return Ok(sink.finish((w, h)));
    }

    let channels = usize::from(reader.output_color_type().0.samples());
    let mut sink = Sink::new(w, h, max_w, max_h);
    let mut y = 0;
    while let Some(row) = reader.next_row().map_err(|e| format!("png: {}", e))? {
        sink.row(y, row.data(), w, channels);
        y += 1;
    }
    Ok(sink.finish((w, h)))
}

// ── JPEG: уменьшение прямо в DCT ───────────────────────────────

fn jpeg(reader: ResourceReader, max_w: u32, max_h: u32) -> Result<Preview, String> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().map_err(|e| format!("jpeg: {}", e))?;
    let source = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;
    let source_dims = (u32::from(source.width), u32::from(source.height));

    // Декодер умеет отдавать кадр уменьшенным (до 1/8 по стороне) — это
    // дешевле, чем развернуть полный и ужать его потом.
    decoder
        .scale(clamp_u16(max_w), clamp_u16(max_h))
        .map_err(|e| format!("jpeg: {}", e))?;
    let pixels = decoder.decode().map_err(|e| format!("jpeg: {}", e))?;
    let info = decoder.info().ok_or_else(|| "jpeg: нет заголовка".to_string())?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));

    let mut sink = Sink::new(w, h, max_w, max_h);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => sink.rows(&pixels, w, h, 1),
        jpeg_decoder::PixelFormat::RGB24 => sink.rows(&pixels, w, h, 3),
        // 16 бит на канал: берём старший байт, как и у остальных форматов.
        jpeg_decoder::PixelFormat::L16 => {
            let bytes: Vec<u8> = pixels.chunks_exact(2).map(|p| p[1]).collect();
            sink.rows(&bytes, w, h, 1);
        }
        other => return Err(format!("jpeg: формат пикселей {:?} не поддерживается", other)),
    }
    Ok(sink.finish(source_dims))
}

// ── TIFF: стрипы/тайлы, с использованием уменьшенных копий ─────

fn tiff(reader: ResourceReader, max_w: u32, max_h: u32) -> Result<Preview, String> {
    let mut decoder = tiff::decoder::Decoder::new(reader).map_err(|e| format!("tiff: {}", e))?;
    let source_dims = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;

    let overview = smallest_overview(&mut decoder, max_w, max_h);
    decoder.seek_to_image(overview.unwrap_or(0)).map_err(|e| format!("tiff: {}", e))?;

    let (w, h) = decoder.dimensions().map_err(|e| format!("tiff: {}", e))?;
    let color = decoder.colortype().map_err(|e| format!("tiff: {}", e))?;
    let channels = tiff_channels(color)?;
    let (chunk_w, chunk_h) = decoder.chunk_dimensions();
    if chunk_w == 0 || chunk_h == 0 {
        return Err("tiff: нулевой размер чанка".to_string());
    }
    let across = w.div_ceil(chunk_w);
    let down = h.div_ceil(chunk_h);

    // Раскладка снимка решает, во что обойдётся превью: с пирамидой читается
    // одна мелкая копия, без неё — все чанки полного разрешения, то есть файл
    // целиком. По логу видно, какой из случаев достался (счётчик трафика —
    // на стороне хоста, network/range.rs).
    veldsdk::log::info!(target: "decode",
        "tiff {}×{} → уровень {}×{}, чанк {}×{}, всего {} (пирамида: {})",
        source_dims.0, source_dims.1, w, h, chunk_w, chunk_h, across * down,
        if overview.is_some() { "есть" } else { "нет" });

    let mut sink = Sink::new(w, h, max_w, max_h);
    for index in 0..across * down {
        let (cw, ch) = decoder.chunk_data_dimensions(index);
        let data = decoder.read_chunk(index).map_err(|e| format!("tiff: {}", e))?;
        let samples = tiff_samples(data)?;
        sink.rect((index % across) * chunk_w, (index / across) * chunk_h, cw, ch, &samples, channels);
    }
    Ok(sink.finish(source_dims))
}

/// Индекс самой мелкой уменьшенной копии, которой ещё хватает на бокс.
///
/// В COG-GeoTIFF пирамида хранится дополнительными IFD, и превью тогда стоит
/// чтения нескольких тайлов вместо всего снимка. Отличаем такие копии по
/// NewSubfileType: в многостраничном TIFF следующие IFD — это отдельные
/// страницы, и подменять ими первую нельзя.
fn smallest_overview<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    max_w: u32,
    max_h: u32,
) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    let mut index = 0;
    while decoder.more_images() {
        if decoder.next_image().is_err() { break; }
        index += 1;

        let reduced = decoder
            .get_tag_unsigned::<u32>(tiff::tags::Tag::NewSubfileType)
            .map(|v| v & 1 != 0)
            .unwrap_or(false);
        if !reduced { continue; }

        let Ok((w, h)) = decoder.dimensions() else { continue };
        if w < max_w || h < max_h { continue; }
        let area = u64::from(w) * u64::from(h);
        if best.is_none_or(|(_, best_area)| area < best_area) {
            best = Some((index, area));
        }
    }
    best.map(|(index, _)| index)
}

fn tiff_channels(color: tiff::ColorType) -> Result<usize, String> {
    match color {
        tiff::ColorType::Gray(_) => Ok(1),
        tiff::ColorType::GrayA(_) => Ok(2),
        tiff::ColorType::RGB(_) => Ok(3),
        tiff::ColorType::RGBA(_) => Ok(4),
        other => Err(format!("tiff: цветовая модель {:?} не поддерживается", other)),
    }
}

/// Сэмплы чанка в 8 битах на канал. У 16-битных снимков берётся старший байт:
/// это ровно то, что нужно для превью, а радиометрическое растягивание —
/// отдельная задача, и придумывать её за пользователя тут нечего.
fn tiff_samples(data: tiff::decoder::DecodingResult) -> Result<Vec<u8>, String> {
    use tiff::decoder::DecodingResult as R;
    match data {
        R::U8(v) => Ok(v),
        R::U16(v) => Ok(v.into_iter().map(|s| (s >> 8) as u8).collect()),
        other => Err(format!("tiff: разрядность сэмплов не поддерживается ({})", match other {
            R::U32(_) => "u32", R::U64(_) => "u64",
            R::F32(_) => "f32", R::F64(_) => "f64",
            _ => "знаковая",
        })),
    }
}

// ── Форматы без потокового пути ────────────────────────────────

fn full(reader: ResourceReader, format: ImageFormat, max_w: u32, max_h: u32) -> Result<Preview, String> {
    let decoder = image::ImageReader::with_format(std::io::BufReader::new(reader), format)
        .into_decoder()
        .map_err(|e| format!("{:?}: {}", format, e))?;
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    let needed = u64::from(w) * u64::from(h) * 4;
    if needed > FULL_DECODE_BUDGET {
        return Err(format!(
            "{:?} {}×{}: кадр целиком не влезает в бюджет ({} МБ)",
            format, w, h, FULL_DECODE_BUDGET / (1024 * 1024)));
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("{:?}: {}", format, e))?
        .to_rgba8();
    let mut sink = Sink::new(w, h, max_w, max_h);
    sink.rows(&rgba, w, h, 4);
    Ok(sink.finish((w, h)))
}

// ── Приёмник пикселей ──────────────────────────────────────────

/// Даунсемплер плюс разворачивание сэмплов в RGBA8. Буфер под развёрнутые
/// пиксели переиспользуется между строками и чанками.
struct Sink {
    inner: Downsampler,
    rgba: Vec<u8>,
}

impl Sink {
    fn new(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> Self {
        Self { inner: Downsampler::new(src_w, src_h, max_w, max_h), rgba: Vec::new() }
    }

    fn row(&mut self, y: u32, samples: &[u8], w: u32, channels: usize) {
        self.rect(0, y, w, 1, samples, channels);
    }

    /// Все строки подряд лежащего буфера.
    fn rows(&mut self, samples: &[u8], w: u32, h: u32, channels: usize) {
        self.rect(0, 0, w, h, samples, channels);
    }

    fn rect(&mut self, x0: u32, y0: u32, w: u32, h: u32, samples: &[u8], channels: usize) {
        let pixels = (w as usize) * (h as usize);
        self.rgba.clear();
        self.rgba.reserve(pixels * 4);
        for px in samples.chunks_exact(channels).take(pixels) {
            let (rgb, alpha) = match channels {
                1 => ([px[0], px[0], px[0]], 255),
                2 => ([px[0], px[0], px[0]], px[1]),
                3 => ([px[0], px[1], px[2]], 255),
                _ => ([px[0], px[1], px[2]], px[3]),
            };
            self.rgba.extend_from_slice(&rgb);
            self.rgba.push(alpha);
        }
        self.inner.add_rect(x0, y0, w, h, &self.rgba);
    }

    fn finish(self, source: (u32, u32)) -> Preview {
        let (width, height) = self.inner.out_size();
        Preview { width, height, source, rgba: self.inner.finish() }
    }
}

fn clamp_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}
