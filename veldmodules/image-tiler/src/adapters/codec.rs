use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::io::{Read, Seek, SeekFrom};

use openjp2::openjpeg::{
    opj_stream_create, opj_stream_destroy, opj_stream_set_read_function,
    opj_stream_set_seek_function, opj_stream_set_skip_function, opj_stream_set_user_data,
    opj_stream_set_user_data_length,
};
use openjp2::{opj_dparameters_t, opj_image, opj_stream_t, Codec, Stream, CODEC_FORMAT};

const BUFFER: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Jp2,
    J2k,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub precision: u32,
    pub signed: bool,
    pub uneven: bool,
}

pub struct Tile<'a> {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub planes: Vec<&'a [i32]>,
}

struct Source<R> {
    reader: R,
}

struct StreamHandle(*mut opj_stream_t);

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // SAFETY: the pointer was issued by `opj_stream_create` and passed to no one.
        unsafe { opj_stream_destroy(self.0) };
    }
}

pub struct Decoder<R: Read + Seek> {
    codec: Codec,
    stream: StreamHandle,
    _source: Box<Source<R>>,
    errors: Box<RefCell<String>>,
    image: Box<opj_image>,
    header: Header,
    dead: bool,
}

impl<R: Read + Seek> Decoder<R> {
    pub fn open(reader: R, len: u64, format: Format, factor: u32) -> Result<Self, String> {
        let kind = match format {
            Format::Jp2 => CODEC_FORMAT::OPJ_CODEC_JP2,
            Format::J2k => CODEC_FORMAT::OPJ_CODEC_J2K,
        };
        let mut codec =
            Codec::new_decoder(kind).ok_or_else(|| "openjp2: codec was not created".to_string())?;
        let errors = Box::new(RefCell::new(String::new()));
        codec.set_error_handler(Some(on_error), &*errors as *const RefCell<String> as *mut c_void);
        let mut parameters = opj_dparameters_t::default();
        if codec.setup_decoder(&mut parameters) != 1 {
            return Err(failure(&errors, "openjp2: decoder did not set up"));
        }
        codec.decoder_set_strict_mode(1);

        let mut source = Box::new(Source { reader });
        // SAFETY: the stream dies with `StreamHandle`; `source` lives in `Decoder` no shorter, and the callbacks cast back to `Source<R>`.
        let mut stream = unsafe {
            let stream = opj_stream_create(BUFFER, 1);
            if stream.is_null() {
                return Err("openjp2: stream was not created".to_string());
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
            None => return Err(failure(&errors, "openjp2: header was not read")),
        };
        let header = header_of(&image)?;
        if factor > 0 && codec.set_decoded_resolution_factor(factor) != 1 {
            return Err(failure(&errors, &format!("openjp2: resolution factor {}", factor)));
        }
        Ok(Self { codec, stream, _source: source, errors, image, header, dead: false })
    }

    pub fn header(&self) -> Header {
        self.header
    }

    pub fn tile<T>(
        &mut self,
        index: u32,
        read: impl FnOnce(&Tile<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.dead {
            return Err("openjp2: codec discarded after a failure".to_string());
        }
        if self.codec.get_decoded_tile(as_stream(&mut self.stream), &mut self.image, index) != 1 {
            self.dead = true;
            return Err(failure(&self.errors, &format!("openjp2: tile {}", index)));
        }
        let comps = self.image.comps().ok_or_else(|| "openjp2: the image has no components".to_string())?;
        let first = comps.first().ok_or_else(|| "openjp2: the image has no components".to_string())?;
        let mut planes = Vec::with_capacity(comps.len());
        for comp in comps {
            if (comp.w, comp.h) != (first.w, first.h) {
                return Err("openjp2: tile components are of different sizes".to_string());
            }
            planes.push(comp.data().ok_or_else(|| "openjp2: tile without data".to_string())?);
        }
        read(&Tile { x0: first.x0, y0: first.y0, width: first.w, height: first.h, planes })
    }
}

fn as_stream(stream: &mut StreamHandle) -> &mut Stream {
    // SAFETY: behind the `opj_stream_create` pointer lies a `Stream`, owned solely by this `StreamHandle`.
    unsafe { &mut *(stream.0 as *mut Stream) }
}

fn header_of(image: &opj_image) -> Result<Header, String> {
    let comps = image.comps().ok_or_else(|| "openjp2: the image has no components".to_string())?;
    let first = comps.first().ok_or_else(|| "openjp2: the image has no components".to_string())?;
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
    // SAFETY: `user` is the `RefCell<String>` passed to `set_error_handler`, living as long as the codec; the message is its C string.
    let (text, errors) = unsafe { (CStr::from_ptr(message).to_string_lossy(), &*(user as *const RefCell<String>)) };
    if let Ok(mut errors) = errors.try_borrow_mut() {
        if !errors.is_empty() {
            errors.push_str("; ");
        }
        errors.push_str(text.trim());
    }
}

unsafe extern "C" fn read<R: Read + Seek>(buffer: *mut c_void, bytes: usize, user: *mut c_void) -> usize {
    // SAFETY: `user` is this decoder's `Source<R>`, and the buffer is issued by the codec for `bytes` bytes.
    let (source, buffer) = unsafe {
        (&mut *(user as *mut Source<R>), std::slice::from_raw_parts_mut(buffer as *mut u8, bytes))
    };
    source.reader.read(buffer).unwrap_or(usize::MAX)
}

unsafe extern "C" fn skip<R: Read + Seek>(bytes: i64, user: *mut c_void) -> i64 {
    // SAFETY: `user` is this decoder's `Source<R>`.
    let source = unsafe { &mut *(user as *mut Source<R>) };
    match source.reader.seek(SeekFrom::Current(bytes)) {
        Ok(_) => bytes,
        Err(_) => -1,
    }
}

unsafe extern "C" fn seek<R: Read + Seek>(offset: i64, user: *mut c_void) -> i32 {
    // SAFETY: `user` is this decoder's `Source<R>`.
    let source = unsafe { &mut *(user as *mut Source<R>) };
    match u64::try_from(offset).map_err(|_| ()).and_then(|at| source.reader.seek(SeekFrom::Start(at)).map_err(|_| ())) {
        Ok(_) => 1,
        Err(()) => 0,
    }
}
