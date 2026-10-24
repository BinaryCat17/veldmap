pub const STRETCH_SAMPLES: usize = 1 << 20;

pub fn is_data(value: f32, nodata: Option<f32>) -> bool {
    value.is_finite() && Some(value) != nodata
}

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

    pub fn get(&self, i: usize) -> f32 {
        match self {
            Samples::U8(v) => f32::from(v[i]),
            Samples::U16(v) => f32::from(v[i]),
            Samples::I16(v) => f32::from(v[i]),
            Samples::F32(v) => v[i],
        }
    }

    fn alpha(&self, i: usize) -> u8 {
        match self {
            Samples::U8(v) => v[i],
            Samples::U16(v) => (v[i] >> 8) as u8,
            Samples::I16(v) => v[i].clamp(0, 255) as u8,
            Samples::F32(v) => (v[i] * 255.0).round().clamp(0.0, 255.0) as u8,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Pixel {
    pub channels: usize,
    colors: usize,
    alpha: bool,
}

impl Pixel {
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

#[derive(Clone, Copy)]
pub struct Mapping {
    stretch: Option<(f32, f32)>,
    nodata: Option<f32>,
}

impl Mapping {
    pub fn identity(nodata: Option<f32>) -> Self {
        Self { stretch: None, nodata }
    }

    pub fn stretched(lo: f32, hi: f32, nodata: Option<f32>) -> Self {
        Self { stretch: Some((lo, hi)), nodata }
    }

    pub fn rgba(&self, samples: &Samples<'_>, pixel: Pixel, pixels: usize) -> Vec<u8> {
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
