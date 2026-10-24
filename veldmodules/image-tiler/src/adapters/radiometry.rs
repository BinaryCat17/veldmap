//! Display radiometry: how TIFF samples become RGBA bytes.
//!
//! u8 is identity: the file is already in bytes. Wide formats (u16, i16, f32)
//! do not fit in a byte, and they share no single "how to squeeze" rule: a DEM
//! lies in meters, radar amplitude in DN with a handful of hot outliers, and
//! the high byte of both yields a black frame. Display stretches the
//! percentiles of the file's own sample: [2%, 98%] → [0, 255], edges clamp.
//! This is a display rule, not data radiometry — measuring by tile bytes is
//! meaningless.
//!
//! "No data" (GDAL_NODATA) is transparency: a pixel whose color channels all
//! equal the declared value gets alpha 0 and black color. The comparison is
//! exact and on the raw value, before the stretch.

/// How many values suffice for a per-file stretch sample.
///
/// A percentile is about the shape of the distribution, not every single
/// value: a million is plenty, while sorting a hundred million would spend
/// more on the stretch than on the whole display. The threshold is shared by
/// everyone who collects this sample: were it to diverge, the same file shown
/// through two paths would stretch differently.
pub const STRETCH_SAMPLES: usize = 1 << 20;

/// Whether a sample is valid: a number and not a "no data" marker.
///
/// One predicate for every place that asks — the TIFF stretch sample, the
/// NetCDF stretch sample, and the display itself. Were they to diverge, they
/// would produce exactly what none of them intends: a value thrown out of the
/// sample as invalid would appear on screen opaque — and, since it lies above
/// the upper edge of the stretch, white. Non-numbers (`NaN`, infinities) are
/// not data by their very meaning: float rasters often mark emptiness with
/// them instead of a tag.
pub fn is_data(value: f32, nodata: Option<f32>) -> bool {
    value.is_finite() && Some(value) != nodata
}

/// Chunk samples in typed form — what the display interprets.
pub enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
    I16(&'a [i16]),
    F32(&'a [f32]),
}

impl Samples<'_> {
    pub fn len(&self) -> usize {
        match self {
            Samples::U8(v) => v.len(),
            Samples::U16(v) => v.len(),
            Samples::I16(v) => v.len(),
            Samples::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sample value as a number — one ladder for stretching and statistics.
    pub fn get(&self, i: usize) -> f32 {
        match self {
            Samples::U8(v) => f32::from(v[i]),
            Samples::U16(v) => f32::from(v[i]),
            Samples::I16(v) => f32::from(v[i]),
            Samples::F32(v) => v[i],
        }
    }

    /// Alpha channel as a byte. Alpha is not stretched — it carries its own
    /// meaning and its own full range of its format.
    fn alpha(&self, i: usize) -> u8 {
        match self {
            Samples::U8(v) => v[i],
            Samples::U16(v) => (v[i] >> 8) as u8,
            Samples::I16(v) => v[i].clamp(0, 255) as u8,
            Samples::F32(v) => (v[i] * 255.0).round().clamp(0.0, 255.0) as u8,
        }
    }
}

/// How to read the samples of one pixel.
///
/// The sample count alone is not enough, and the difference is not subtle. Two
/// samples in a named color model are grey with alpha, while two samples in a
/// stack of measured quantities are two quantities, and the second one is not
/// transparency: read as alpha, it would black out half the scene. So what
/// decides is not the count but what the file calls itself.
#[derive(Clone, Copy)]
pub struct Pixel {
    /// How many samples fall on one pixel — the stride through the array.
    pub channels: usize,
    /// How many of the leading ones make up the color.
    colors: usize,
    /// The last sample is transparency.
    alpha: bool,
}

impl Pixel {
    /// The model named by the file: grey, grey with alpha, RGB, RGBA.
    pub fn named(channels: usize) -> Self {
        Self {
            channels,
            colors: match channels {
                1 | 2 => 1,
                _ => 3,
            },
            alpha: matches!(channels, 2 | 4),
        }
    }

    /// A stack of measured quantities without color interpretation — how GDAL
    /// writes anything that is not three or four color channels. The first one
    /// is shown: which of them is the image, the file does not say, and
    /// combining them into color would pass a guess off as measured data.
    pub fn stack(channels: usize) -> Self {
        Self { channels, colors: 1, alpha: false }
    }

    pub fn colors(&self) -> usize {
        self.colors
    }

    pub fn has_alpha(&self) -> bool {
        self.alpha
    }
}

/// Display mapping for one file. Built once per file (see tiff.rs) and
/// applied to all its chunks — otherwise adjacent tiles would each stretch
/// their own way and part at the seams.
///
/// Copies for free: two bounds and the "no data" value. That is what lets it
/// be kept ready at parse time instead of recomputed for every request.
#[derive(Clone, Copy)]
pub struct Mapping {
    /// Stretch [lo, hi] → [0, 255]; `None` — the samples are already bytes.
    stretch: Option<(f32, f32)>,
    /// The "no data" value — transparency.
    nodata: Option<f32>,
}

impl Mapping {
    pub fn identity(nodata: Option<f32>) -> Self {
        Self { stretch: None, nodata }
    }

    /// `hi > lo` is the caller's concern (see [`percentile_stretch`]).
    pub fn stretched(lo: f32, hi: f32, nodata: Option<f32>) -> Self {
        Self { stretch: Some((lo, hi)), nodata }
    }

    /// Unfolds samples into RGBA8 by the pixel layout (see [`Pixel`]); the tail
    /// beyond `pixels` is dropped.
    pub fn rgba(&self, samples: &Samples<'_>, pixel: Pixel, pixels: usize) -> Vec<u8> {
        // Bytes without stretch or keying — a ready path with no recomputation.
        if let (Samples::U8(v), None, None) = (samples, self.stretch, self.nodata) {
            return super::to_rgba(v, pixel, pixels);
        }

        let (lo, hi) = self.stretch.unwrap_or((0.0, 255.0));
        let scale = 255.0 / (hi - lo);
        let Pixel { channels, colors, alpha: has_alpha } = pixel;

        let count = pixels.min(samples.len() / channels);
        let mut rgba = Vec::with_capacity(count * 4);
        for px in 0..count {
            let at = px * channels;
            // Two distinct circumstances make a pixel empty, and they must not
            // be confused. Every color channel is the "no data" marker: this
            // is a field beyond the edge of the capture. Any one channel is not
            // a number: such a pixel has no color at all, and one non-numeric
            // channel spoils it entirely.
            let mut void = true;
            let mut broken = false;
            let mut bytes = [0u8; 3];
            for c in 0..colors {
                let v = samples.get(at + c);
                broken |= !v.is_finite();
                void &= !is_data(v, self.nodata);
                bytes[c] = ((v - lo) * scale).round().clamp(0.0, 255.0) as u8;
            }
            if void || broken {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            if colors == 1 {
                (bytes[1], bytes[2]) = (bytes[0], bytes[0]);
            }
            rgba.extend_from_slice(&bytes);
            rgba.push(if has_alpha { samples.alpha(at + channels - 1) } else { 255 });
        }
        rgba
    }
}

/// Stretch from the file's own sample: the [2nd, 98th] percentiles of valid
/// values. `None` — the sample is empty (the whole file is "no data" — nothing
/// to stretch).
///
/// Coinciding percentiles mean a flat field, and it is opened **around** the
/// value, not upward from it: upward would mean the whole field sits at the
/// lower edge of the scale, that is a black frame indistinguishable from
/// emptiness. A flat field belongs in the middle.
///
/// The step is taken from the magnitude of the value itself, not a unit: for
/// f32 beyond 2²⁴ adding one does not change the number at all, and division
/// by zero returns — and exactly such values stand as "no data" markers (the
/// standard netCDF `_FillValue` is 9.96921e36).
pub fn percentile_stretch(values: &mut [f32]) -> Option<(f32, f32)> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f32::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    let (lo, hi) = (at(0.02), at(0.98));
    if hi > lo {
        return Some((lo, hi));
    }
    let half = lo.abs().max(1.0) * 1e-6 * 0.5;
    Some((lo - half, lo + half))
}
