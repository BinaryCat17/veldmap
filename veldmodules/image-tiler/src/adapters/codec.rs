//! openjp2 за одним unsafe-островом: поток поверх читателя ресурса через
//! C-колбэки, обработчик ошибок и декод одного тайла кодстрима на факторе
//! разрешения ([`Decoder`]). Порт C с сырыми указателями читает данные из
//! сети — отсюда три правила: обработчик ошибок стои́т всегда (без него у
//! отказа нет причины), после любого отказа кодек выбрасывается (его
//! состояние после отказа не определено), а разбор запускается только на
//! файле, который собираются показывать (`assert!` в разборе — трап
//! инстанса). Всё `unsafe` тайлера — здесь; наружу уходят срезы `i32`.

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::io::{Read, Seek, SeekFrom};

use openjp2::openjpeg::{
    opj_stream_create, opj_stream_destroy, opj_stream_set_read_function,
    opj_stream_set_seek_function, opj_stream_set_skip_function, opj_stream_set_user_data,
    opj_stream_set_user_data_length,
};
use openjp2::{opj_dparameters_t, opj_image, opj_stream_t, Codec, Stream, CODEC_FORMAT};

/// Сколько байт кодек просит у читателя за раз. Читатель ресурса держит своё
/// окно, и этот буфер лишь режет вызовы: меньший плодил бы их, больший читал
/// бы за конец тайл-парта то, что никому не нужно.
const BUFFER: usize = 64 * 1024;

/// Что лежит в файле: контейнер JP2 или сырой кодстрим.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Jp2,
    J2k,
}

/// Что кодек прочёл из главного заголовка.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub components: u32,
    /// Разрядность отсчёта первого компонента и знаковость.
    pub precision: u32,
    pub signed: bool,
    /// Компоненты разной решётки или разрядности — такой тайл в RGBA не
    /// разложить.
    pub uneven: bool,
}

/// Декодированный тайл на факторе разрешения: его начало в координатах
/// полного холста (так его отдаёт кодек), размеры на уменьшенной решётке и
/// плоскости компонентов, по одной на компонент, `width × height` отсчётов.
pub struct Tile<'a> {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub planes: Vec<&'a [i32]>,
}

/// Читатель за указателем пользовательских данных потока.
struct Source<R> {
    reader: R,
}

/// Поток кодека — уничтожается вместе с владельцем, на всяком выходе.
struct StreamHandle(*mut opj_stream_t);

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // SAFETY: указатель выдан `opj_stream_create` и никому не передан.
        unsafe { opj_stream_destroy(self.0) };
    }
}

/// Декодер одного файла на одном факторе разрешения. Фактор — свойство
/// открытия: сменить его значит открыть заново, состояние кодека между
/// тайлами не переиспользуется иначе как для следующего тайла.
pub struct Decoder<R: Read + Seek> {
    codec: Codec,
    stream: StreamHandle,
    /// Держится ради указателя в потоке: живёт столько же, сколько поток.
    _source: Box<Source<R>>,
    errors: Box<RefCell<String>>,
    image: Box<opj_image>,
    header: Header,
    /// После отказа кодек не трогают: он выброшен.
    dead: bool,
}

impl<R: Read + Seek> Decoder<R> {
    /// Открывает файл длиной `len` байт и читает главный заголовок; `factor` —
    /// уровней разрешения, на сколько тайлы отдаются грубее родного.
    pub fn open(reader: R, len: u64, format: Format, factor: u32) -> Result<Self, String> {
        let kind = match format {
            Format::Jp2 => CODEC_FORMAT::OPJ_CODEC_JP2,
            Format::J2k => CODEC_FORMAT::OPJ_CODEC_J2K,
        };
        let mut codec =
            Codec::new_decoder(kind).ok_or_else(|| "openjp2: кодек не создался".to_string())?;
        let errors = Box::new(RefCell::new(String::new()));
        codec.set_error_handler(Some(on_error), &*errors as *const RefCell<String> as *mut c_void);
        let mut parameters = opj_dparameters_t::default();
        if codec.setup_decoder(&mut parameters) != 1 {
            return Err(failure(&errors, "openjp2: декодер не настроился"));
        }
        codec.decoder_set_strict_mode(1);

        let mut source = Box::new(Source { reader });
        // SAFETY: поток создан здесь и уничтожается вместе с `StreamHandle`;
        // пользовательские данные — `source`, который живёт в `Decoder` не
        // меньше потока; колбэки приводят указатель к тому же типу `R`.
        let mut stream = unsafe {
            let stream = opj_stream_create(BUFFER, 1);
            if stream.is_null() {
                return Err("openjp2: поток не создался".to_string());
            }
            opj_stream_set_user_data(stream, &mut *source as *mut Source<R> as *mut c_void, None);
            opj_stream_set_user_data_length(stream, len);
            opj_stream_set_read_function(stream, Some(read::<R>));
            opj_stream_set_skip_function(stream, Some(skip::<R>));
            opj_stream_set_seek_function(stream, Some(seek::<R>));
            StreamHandle(stream)
        };

        let image = match codec.read_header(as_stream(&mut stream)) {
            Some(image) => image,
            None => return Err(failure(&errors, "openjp2: заголовок не прочитался")),
        };
        let header = header_of(&image)?;
        if factor > 0 && codec.set_decoded_resolution_factor(factor) != 1 {
            return Err(failure(&errors, &format!("openjp2: фактор разрешения {}", factor)));
        }
        Ok(Self { codec, stream, _source: source, errors, image, header, dead: false })
    }

    pub fn header(&self) -> Header {
        self.header
    }

    /// Декодирует тайл `index` (ряд × тайлов в ряду + колонка) и отдаёт его
    /// плоскости `read`, пока они живы. Отказ выбрасывает кодек: следующий
    /// вызов отвечает отказом сразу.
    pub fn tile<T>(
        &mut self,
        index: u32,
        read: impl FnOnce(&Tile<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.dead {
            return Err("openjp2: кодек выброшен после отказа".to_string());
        }
        if self.codec.get_decoded_tile(as_stream(&mut self.stream), &mut self.image, index) != 1 {
            self.dead = true;
            return Err(failure(&self.errors, &format!("openjp2: тайл {}", index)));
        }
        let comps = self.image.comps().ok_or_else(|| "openjp2: у образа нет компонентов".to_string())?;
        let first = comps.first().ok_or_else(|| "openjp2: у образа нет компонентов".to_string())?;
        let mut planes = Vec::with_capacity(comps.len());
        for comp in comps {
            if (comp.w, comp.h) != (first.w, first.h) {
                return Err("openjp2: компоненты тайла разного размера".to_string());
            }
            planes.push(comp.data().ok_or_else(|| "openjp2: тайл без данных".to_string())?);
        }
        read(&Tile { x0: first.x0, y0: first.y0, width: first.w, height: first.h, planes })
    }
}

/// Поток, каким его просят методы кодека. Через `&mut`: ссылка живёт не
/// дольше заёма владельца, и двух её разом не бывает.
fn as_stream(stream: &mut StreamHandle) -> &mut Stream {
    // SAFETY: за указателем `opj_stream_create` лежит `Stream`, владеет им
    // только этот `StreamHandle`, и заём владельца исключает второй доступ.
    unsafe { &mut *(stream.0 as *mut Stream) }
}

fn header_of(image: &opj_image) -> Result<Header, String> {
    let comps = image.comps().ok_or_else(|| "openjp2: у образа нет компонентов".to_string())?;
    let first = comps.first().ok_or_else(|| "openjp2: у образа нет компонентов".to_string())?;
    let uneven = comps
        .iter()
        .any(|comp| (comp.dx, comp.dy, comp.prec, comp.sgnd) != (first.dx, first.dy, first.prec, first.sgnd))
        || first.dx != 1
        || first.dy != 1;
    Ok(Header {
        x0: image.x0,
        y0: image.y0,
        width: image.x1.saturating_sub(image.x0),
        height: image.y1.saturating_sub(image.y0),
        components: image.numcomps,
        precision: first.prec,
        signed: first.sgnd != 0,
        uneven,
    })
}

/// Отказ с причиной, которую назвал кодек: без обработчика ошибок её нет.
fn failure(errors: &RefCell<String>, what: &str) -> String {
    let said = errors.borrow().trim().to_string();
    match said.is_empty() {
        true => what.to_string(),
        false => format!("{}: {}", what, said),
    }
}

unsafe extern "C" fn on_error(message: *const c_char, user: *mut c_void) {
    if message.is_null() || user.is_null() {
        return;
    }
    // SAFETY: `user` — тот `RefCell<String>`, что передан `set_error_handler`,
    // и живёт столько же, сколько кодек; сообщение — C-строка кодека.
    let (text, errors) = unsafe { (CStr::from_ptr(message).to_string_lossy(), &*(user as *const RefCell<String>)) };
    // Паника за границей `extern "C"` — это abort; заём здесь всегда свободен,
    // и `try_borrow_mut` лишь делает это правилом, а не надеждой.
    if let Ok(mut errors) = errors.try_borrow_mut() {
        if !errors.is_empty() {
            errors.push_str("; ");
        }
        errors.push_str(text.trim());
    }
}

unsafe extern "C" fn read<R: Read + Seek>(buffer: *mut c_void, bytes: usize, user: *mut c_void) -> usize {
    // SAFETY: `user` — `Source<R>` этого декодера; буфер выдан кодеком на
    // `bytes` байт. Ноль — конец файла, как у `Read`; `usize::MAX` — отказ.
    let (source, buffer) = unsafe {
        (&mut *(user as *mut Source<R>), std::slice::from_raw_parts_mut(buffer as *mut u8, bytes))
    };
    source.reader.read(buffer).unwrap_or(usize::MAX)
}

unsafe extern "C" fn skip<R: Read + Seek>(bytes: i64, user: *mut c_void) -> i64 {
    // SAFETY: `user` — `Source<R>` этого декодера.
    let source = unsafe { &mut *(user as *mut Source<R>) };
    match source.reader.seek(SeekFrom::Current(bytes)) {
        Ok(_) => bytes,
        Err(_) => -1,
    }
}

unsafe extern "C" fn seek<R: Read + Seek>(offset: i64, user: *mut c_void) -> i32 {
    // SAFETY: `user` — `Source<R>` этого декодера.
    let source = unsafe { &mut *(user as *mut Source<R>) };
    match u64::try_from(offset).map_err(|_| ()).and_then(|at| source.reader.seek(SeekFrom::Start(at)).map_err(|_| ())) {
        Ok(_) => 1,
        Err(()) => 0,
    }
}

/// Энкодер для фикстур: тайловый кодстрим J2K в память, RGB8 без потерь
/// (обратимое вейвлет 5/3). Здесь же, потому что поток вывода — тот же
/// C-ABI, что у чтения, и второго острова заводить незачем.
#[cfg(test)]
pub(super) mod fixture {
    use std::ffi::c_void;
    use std::io::{Cursor, Seek, SeekFrom, Write};

    use openjp2::openjpeg::{
        opj_stream_create, opj_stream_destroy, opj_stream_set_seek_function,
        opj_stream_set_skip_function, opj_stream_set_user_data, opj_stream_set_write_function,
    };
    use openjp2::{opj_cparameters_t, opj_image, opj_image_comptparm, Codec, Stream, CODEC_FORMAT, COLOR_SPACE};

    /// Кодстрим `width`×`height` RGB8 тайлами `tile`×`tile` с `resolutions`
    /// уровнями разрешения; `rgb` — отсчёты построчно.
    pub fn tiled_j2k(width: u32, height: u32, tile: u32, resolutions: u32, rgb: &[u8]) -> Vec<u8> {
        assert_eq!(rgb.len(), (width as usize) * (height as usize) * 3);
        let planes: Vec<Vec<i32>> =
            (0..3).map(|channel| rgb.chunks_exact(3).map(|px| i32::from(px[channel])).collect()).collect();
        encode(width, height, tile, resolutions, 8, &planes, COLOR_SPACE::OPJ_CLRSPC_SRGB)
    }

    /// Серый кодстрим шире байта: один компонент разрядности `prec`, отсчёты
    /// построчно — так лежат полосы Sentinel-2 (B04 и прочие, кроме TCI).
    pub fn gray_j2k(width: u32, height: u32, tile: u32, resolutions: u32, prec: u32, samples: &[u16]) -> Vec<u8> {
        assert_eq!(samples.len(), (width as usize) * (height as usize));
        let plane: Vec<i32> = samples.iter().map(|s| i32::from(*s)).collect();
        encode(width, height, tile, resolutions, prec, &[plane], COLOR_SPACE::OPJ_CLRSPC_GRAY)
    }

    fn encode(width: u32, height: u32, tile: u32, resolutions: u32, prec: u32, planes: &[Vec<i32>], space: COLOR_SPACE) -> Vec<u8> {
        let component = opj_image_comptparm { dx: 1, dy: 1, w: width, h: height, x0: 0, y0: 0, prec, bpp: prec, sgnd: 0 };
        let components: Vec<opj_image_comptparm> = planes.iter().map(|_| component).collect();
        let mut image = opj_image::create(&components, space).expect("образ создаётся");
        // Границы холста образ не выводит из компонентов — их ставит тот, кто
        // кодирует, как это делает и сам OpenJPEG.
        image.x0 = 0;
        image.y0 = 0;
        image.x1 = width;
        image.y1 = height;
        for (source, plane) in planes.iter().zip(image.comps_data_mut_iter().expect("компоненты есть")) {
            plane.copy_from_slice(source);
        }

        let mut parameters = opj_cparameters_t::default();
        parameters.tile_size_on = 1;
        parameters.cp_tdx = tile as i32;
        parameters.cp_tdy = tile as i32;
        parameters.numresolution = resolutions as i32;
        parameters.tcp_numlayers = 1;
        parameters.tcp_rates[0] = 0.0;
        parameters.cp_disto_alloc = 1;
        parameters.irreversible = 0;

        let mut codec = Codec::new_encoder(CODEC_FORMAT::OPJ_CODEC_J2K).expect("энкодер создаётся");
        assert_eq!(codec.setup_encoder(&mut parameters, &mut image), 1, "энкодер настраивается");

        let mut out = Box::new(Cursor::new(Vec::new()));
        // SAFETY: поток создан здесь и уничтожен ниже; пользовательские данные
        // — `out`, живущий дольше потока; колбэки приводят указатель к нему.
        let stream = unsafe {
            let stream = opj_stream_create(64 * 1024, 0);
            assert!(!stream.is_null());
            opj_stream_set_user_data(stream, &mut *out as *mut Cursor<Vec<u8>> as *mut c_void, None);
            opj_stream_set_write_function(stream, Some(write));
            opj_stream_set_skip_function(stream, Some(skip));
            opj_stream_set_seek_function(stream, Some(seek));
            stream
        };
        // SAFETY: за указателем лежит `Stream`.
        let as_stream = || unsafe { &mut *(stream as *mut Stream) };
        assert_eq!(codec.start_compress(&mut image, as_stream()), 1, "сжатие начинается");
        assert_eq!(codec.encode(as_stream()), 1, "кодстрим пишется");
        assert_eq!(codec.end_compress(as_stream()), 1, "кодстрим закрывается");
        // SAFETY: указатель выдан `opj_stream_create` и больше не используется.
        unsafe { opj_stream_destroy(stream) };
        drop(codec);
        out.into_inner()
    }

    unsafe extern "C" fn write(buffer: *mut c_void, bytes: usize, user: *mut c_void) -> usize {
        // SAFETY: `user` — курсор фикстуры, буфер выдан кодеком на `bytes` байт.
        let (out, buffer) = unsafe {
            (&mut *(user as *mut Cursor<Vec<u8>>), std::slice::from_raw_parts(buffer as *const u8, bytes))
        };
        out.write_all(buffer).map(|()| bytes).unwrap_or(usize::MAX)
    }

    unsafe extern "C" fn skip(bytes: i64, user: *mut c_void) -> i64 {
        // SAFETY: `user` — курсор фикстуры.
        let out = unsafe { &mut *(user as *mut Cursor<Vec<u8>>) };
        match out.seek(SeekFrom::Current(bytes)) {
            Ok(_) => bytes,
            Err(_) => -1,
        }
    }

    unsafe extern "C" fn seek(offset: i64, user: *mut c_void) -> i32 {
        // SAFETY: `user` — курсор фикстуры.
        let out = unsafe { &mut *(user as *mut Cursor<Vec<u8>>) };
        match out.seek(SeekFrom::Start(offset as u64)) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn pattern(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|at| {
                let (x, y) = (at % width, at / width);
                [(x * 7 % 251) as u8, (y * 13 % 241) as u8, ((x ^ y) % 229) as u8]
            })
            .collect()
    }

    /// Обратимый кодстрим декодируется тайл за тайлом байт в байт — на любом
    /// факторе размеры тайла делятся пополам вверх, как у пирамиды.
    #[test]
    fn a_reversible_tile_decodes_byte_for_byte() {
        let (w, h, tile) = (300u32, 200u32, 128u32);
        let rgb = pattern(w, h);
        let bytes = fixture::tiled_j2k(w, h, tile, 3, &rgb);
        let len = bytes.len() as u64;

        let mut decoder = Decoder::open(Cursor::new(bytes.clone()), len, Format::J2k, 0).expect("открывается");
        let header = decoder.header();
        assert_eq!((header.width, header.height, header.components, header.precision), (w, h, 3, 8));
        assert!(!header.signed && !header.uneven);

        // Тайл (1, 1) — второй ряд, вторая колонка: 128×72 пикселей.
        let across = w.div_ceil(tile);
        decoder
            .tile(1 * across + 1, |got| {
                assert_eq!((got.x0, got.y0, got.width, got.height), (128, 128, 128, 72));
                for y in 0..got.height {
                    for x in 0..got.width {
                        let at = (((got.y0 + y) * w) + got.x0 + x) as usize;
                        for c in 0..3 {
                            assert_eq!(got.planes[c][(y * got.width + x) as usize], i32::from(rgb[at * 3 + c]), "пиксель {x},{y} канал {c}");
                        }
                    }
                }
                Ok(())
            })
            .expect("тайл декодируется");

        let mut coarse = Decoder::open(Cursor::new(bytes), len, Format::J2k, 2).expect("открывается на факторе 2");
        coarse
            .tile(0, |got| {
                assert_eq!((got.width, got.height), (32, 32), "тайл 128² на факторе 2");
                Ok(())
            })
            .expect("грубый тайл декодируется");
    }

    /// Отказ называет причину кодека и выбрасывает его: следующий тайл —
    /// отказ сразу, без обращения к кодеку.
    #[test]
    fn a_failure_names_the_cause_and_discards_the_codec() {
        let rgb = pattern(64, 64);
        let mut bytes = fixture::tiled_j2k(64, 64, 64, 2, &rgb);
        let len = bytes.len() as u64;
        bytes.truncate(bytes.len() / 2);

        let mut decoder = Decoder::open(Cursor::new(bytes), len, Format::J2k, 0).expect("заголовок целый");
        let why = decoder.tile(0, |_| Ok(())).expect_err("обрезанный тайл не декодируется");
        assert!(why.starts_with("openjp2: тайл 0"), "{why}");
        let again = decoder.tile(0, |_| Ok(())).expect_err("кодек выброшен");
        assert!(again.contains("выброшен"), "{again}");
    }

    /// Мусор вместо кодстрима — отказ заголовка с причиной, а не паника.
    #[test]
    fn garbage_is_refused_at_the_header() {
        let junk = vec![0xFFu8, 0x4F, 0xFF, 0x51, 0, 0, 0, 0, 1, 2, 3];
        let why = Decoder::open(Cursor::new(junk.clone()), junk.len() as u64, Format::J2k, 0)
            .err()
            .expect("мусор не открывается");
        assert!(why.contains("заголовок"), "{why}");
    }
}

