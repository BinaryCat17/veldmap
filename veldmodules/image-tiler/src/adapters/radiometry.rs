//! Радиометрия показа: как сэмплы TIFF становятся байтами RGBA.
//!
//! u8 — тождество: файл уже в байтах. Широкие форматы (u16, i16, f32) в байт
//! не влезают, а одного правила «как ужать» у них нет: DEM лежит в метрах,
//! радарная амплитуда — в DN с горсткой горячих выбросов, и старший байт у
//! обоих даёт чёрный кадр. Показ растягивает перцентили выборки самого файла:
//! [2%, 98%] → [0, 255], края клампятся. Это правило показа, а не радиометрия
//! данных — измерять по байтам тайла нельзя.
//!
//! «Нет данных» (GDAL_NODATA) — прозрачность: пиксель, все цветовые каналы
//! которого равны объявленному значению, получает альфу 0 и чёрный цвет.
//! Сравнение точное и по сырому значению, до растяга.

/// Сколько значений хватает выборке растяга на файл.
///
/// Перцентиль — про форму распределения, а не про каждое значение: миллиона
/// довольно, а сортировать сотню миллионов значит потратить на растяг больше,
/// чем на весь показ. Порог один на всех, кто эту выборку собирает: разойдись
/// он — и один и тот же файл, показанный двумя путями, растянулся бы
/// по-разному.
pub const STRETCH_SAMPLES: usize = 1 << 20;

/// Годен ли отсчёт: число и не метка «нет данных».
///
/// Один предикат на все места, где об этом спрашивают, — выборку растяга у
/// TIFF, выборку растяга у NetCDF и сам показ. Разойдясь, они дают ровно то,
/// чего не хотел ни один: значение, выброшенное из выборки как негодное,
/// выходит на экран непрозрачным — и, поскольку выше верхнего края растяга,
/// белым. Нечисло (`NaN`, бесконечности) не данные по самому своему смыслу:
/// float-растры нередко метят им пустоту вместо тега.
pub fn is_data(value: f32, nodata: Option<f32>) -> bool {
    value.is_finite() && Some(value) != nodata
}

/// Сэмплы чанка в типизированном виде — то, что разбирает показ.
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

    /// Значение сэмпла числом — одна лестница для растяга и статистики.
    pub fn get(&self, i: usize) -> f32 {
        match self {
            Samples::U8(v) => f32::from(v[i]),
            Samples::U16(v) => f32::from(v[i]),
            Samples::I16(v) => f32::from(v[i]),
            Samples::F32(v) => v[i],
        }
    }

    /// Альфа-канал как байт. Альфу не растягивают — у неё собственный смысл
    /// и собственный полный диапазон своего формата.
    fn alpha(&self, i: usize) -> u8 {
        match self {
            Samples::U8(v) => v[i],
            Samples::U16(v) => (v[i] >> 8) as u8,
            Samples::I16(v) => v[i].clamp(0, 255) as u8,
            Samples::F32(v) => (v[i] * 255.0).round().clamp(0.0, 255.0) as u8,
        }
    }
}

/// Как читать сэмплы одного пикселя.
///
/// Числа сэмплов для этого мало, и разница не тонкая. Два сэмпла у названной
/// цветовой модели — это серый с альфой, а два сэмпла у стопки измеренных
/// величин — две величины, и вторая из них не прозрачность: прочитанная альфой,
/// она погасила бы половину снимка. Поэтому решает не счёт, а то, чем файл себя
/// назвал.
#[derive(Clone, Copy)]
pub struct Pixel {
    /// Сколько сэмплов приходится на пиксель — шаг по массиву.
    pub channels: usize,
    /// Сколько первых из них составляют цвет.
    colors: usize,
    /// Последний сэмпл — прозрачность.
    alpha: bool,
}

impl Pixel {
    /// Модель, названная файлом: серый, серый с альфой, RGB, RGBA.
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

    /// Стопка измеренных величин без цветовой интерпретации — так GDAL пишет
    /// всё, что не три и не четыре канала цвета. Показывается первая: какая из
    /// них изображение, файл не говорит, а сложить их в цвет значило бы выдать
    /// догадку за снятое.
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

/// Маппинг показа одного файла. Строится один раз на файл (см. tiff.rs) и
/// применяется ко всем его чанкам — иначе соседние тайлы растянулись бы
/// каждый по-своему и разошлись швами.
pub struct Mapping {
    /// Растяг [lo, hi] → [0, 255]; `None` — сэмплы уже байты.
    stretch: Option<(f32, f32)>,
    /// Значение «нет данных» — прозрачность.
    nodata: Option<f32>,
}

impl Mapping {
    pub fn identity(nodata: Option<f32>) -> Self {
        Self { stretch: None, nodata }
    }

    /// `hi > lo` — забота вызывающего (см. [`percentile_stretch`]).
    pub fn stretched(lo: f32, hi: f32, nodata: Option<f32>) -> Self {
        Self { stretch: Some((lo, hi)), nodata }
    }

    /// Развёртка сэмплов в RGBA8 по раскладке пикселя (см. [`Pixel`]); хвост за
    /// пределами `pixels` отбрасывается.
    pub fn rgba(&self, samples: &Samples<'_>, pixel: Pixel, pixels: usize) -> Vec<u8> {
        // Байты без растяга и ключевания — готовый путь без пересчёта.
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
            // Пустым пиксель делают два разных обстоятельства, и путать их
            // нельзя. Все цветовые каналы — метка «нет данных»: это поле за
            // краем снятого. Хоть один канал не число: цвета у такого пикселя
            // нет вовсе, и один нечисловой канал портит его целиком.
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

/// Растяг по выборке файла: [2-й, 98-й] перцентили валидных значений.
/// `None` — выборка пуста (весь файл «нет данных» — растягивать нечего).
///
/// Совпавшие перцентили — это ровное поле, и раздвигается оно **вокруг**
/// значения, а не вверх от него: вверх означало бы, что всё поле лежит у
/// нижнего края шкалы, то есть чёрный кадр, неотличимый от пустоты. Ровному
/// полю место посередине.
///
/// Шаг берётся от величины самого значения, а не единицей: у f32 за 2²⁴
/// прибавка единицы не меняет числа вовсе, и деление на ноль возвращается —
/// а именно такие значения и стоят метками «нет данных» (штатное `_FillValue`
/// netCDF — 9,96921·10³⁶).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_bytes_match_plain_expansion() {
        let mapping = Mapping::identity(None);
        let bytes = [7u8, 9, 200];
        assert_eq!(
            mapping.rgba(&Samples::U8(&bytes), Pixel::named(1), 3),
            super::super::to_rgba(&bytes, Pixel::named(1), 3)
        );
    }

    #[test]
    fn nodata_pixel_turns_transparent_black() {
        // Серый u8 с nodata 7: первый пиксель — поле, второй — данные.
        let mapping = Mapping::identity(Some(7.0));
        let out = mapping.rgba(&Samples::U8(&[7, 9]), Pixel::named(1), 2);
        assert_eq!(out, vec![0, 0, 0, 0, 9, 9, 9, 255]);
    }

    #[test]
    fn nodata_keys_only_when_every_colour_matches() {
        // RGB: (0,0,0) — поле, (0,0,1) — законный почти-чёрный.
        let mapping = Mapping::stretched(0.0, 255.0, Some(0.0));
        let out = mapping.rgba(&Samples::U8(&[0, 0, 0, 0, 0, 1]), Pixel::named(3), 2);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert_eq!(&out[4..], &[0, 0, 1, 255]);
    }

    #[test]
    fn stretch_maps_range_and_clamps_edges() {
        let mapping = Mapping::stretched(100.0, 355.0, None);
        let out = mapping.rgba(&Samples::U16(&[100, 355, 50, 1000, 22750]), Pixel::named(1), 4);
        assert_eq!(&out[0..4], &[0, 0, 0, 255]); // lo → 0
        assert_eq!(&out[4..8], &[255, 255, 255, 255]); // hi → 255
        assert_eq!(&out[8..12], &[0, 0, 0, 255]); // ниже lo — кламп
        assert_eq!(&out[12..16], &[255, 255, 255, 255]); // выше hi — кламп
    }

    #[test]
    fn f32_nodata_is_compared_raw_before_stretch() {
        // DEM: nodata −32767 не участвует ни в цвете, ни в растяге.
        let mapping = Mapping::stretched(0.0, 100.0, Some(-32767.0));
        let out = mapping.rgba(&Samples::F32(&[-32767.0, 50.0]), Pixel::named(1), 2);
        assert_eq!(out, vec![0, 0, 0, 0, 128, 128, 128, 255]);
    }

    #[test]
    fn nan_sample_is_transparent_even_without_declared_nodata() {
        let mapping = Mapping::stretched(0.0, 100.0, None);
        let out = mapping.rgba(&Samples::F32(&[f32::NAN, 50.0]), Pixel::named(1), 2);
        assert_eq!(out, vec![0, 0, 0, 0, 128, 128, 128, 255]);
    }

    #[test]
    fn alpha_channel_is_scaled_not_stretched() {
        // Серый с альфой u16: цвет растягивается, альфа — старший байт.
        let mapping = Mapping::stretched(0.0, 1000.0, None);
        let out = mapping.rgba(&Samples::U16(&[500, 0x8000]), Pixel::named(2), 1);
        assert_eq!(out, vec![128, 128, 128, 0x80]);
    }

    #[test]
    fn percentiles_ignore_hot_outliers() {
        // 0..=100 с одиночным выбросом: растяг остаётся у тела распределения.
        let mut values: Vec<f32> = (0..=100).map(|v| v as f32).collect();
        values.push(1_000_000.0);
        let (lo, hi) = percentile_stretch(&mut values).unwrap();
        assert_eq!(lo, 2.0);
        assert!((98.0..=100.0).contains(&hi), "hi = {}", hi);
    }

    /// Ровное поле встаёт посередине шкалы, а не в её нижний край: чёрный
    /// кадр неотличим от пустоты, а поле однотонно, а не пусто.
    #[test]
    fn flat_field_lands_in_the_middle_of_the_scale() {
        for value in [0.0f32, 42.0, -273.15, 1.0e7, 9.969_21e36] {
            let mut values = vec![value; 10];
            let (lo, hi) = percentile_stretch(&mut values).expect("выборка не пуста");
            assert!(hi > lo, "растяг вырожден при {}: {}..{}", value, lo, hi);
            let grey = Mapping::stretched(lo, hi, None)
                .rgba(&Samples::F32(&[value]), Pixel::named(1), 1);
            assert!((100..=160).contains(&grey[0]), "{} дало яркость {}", value, grey[0]);
            assert_eq!(grey[3], 255, "ровное поле не пусто");
        }
        assert_eq!(percentile_stretch(&mut []), None);
    }

    /// Один и тот же вопрос — годен ли отсчёт — у выборки растяга и у показа.
    /// Разойдясь, они дают белый непрозрачный пиксель там, где выборка
    /// посчитала значение негодным.
    #[test]
    fn what_the_sample_throws_out_the_frame_does_not_show() {
        let nodata = Some(-9999.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -9999.0] {
            assert!(!is_data(bad, nodata), "{} сочтено данными", bad);
            let out = Mapping::stretched(0.0, 100.0, nodata)
                .rgba(&Samples::F32(&[bad]), Pixel::named(1), 1);
            assert_eq!(&out[..], &[0, 0, 0, 0], "{} вышло непрозрачным", bad);
        }
        assert!(is_data(0.0, nodata) && is_data(-9999.0, None));
    }
}
