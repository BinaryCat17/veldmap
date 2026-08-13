//! Каскад полос: все уровни пирамиды за один проход по источнику.
//!
//! Вход — полнокровные группы строк базового уровня, сверху вниз; выход —
//! готовые тайлы каждого уровня через колбэк, в момент, когда полоса уровня
//! дозаполнилась. Полоса — это `TILE` строк уровня: заполнилась → нарезана в
//! тайлы и ужата вдвое в полосу следующего уровня. Память — по одной полосе
//! на уровень, геометрическая прогрессия от ширины: ≈ `width × TILE × 4 × 2`
//! байт, от высоты источника не зависит.
//!
//! Границы полос выровнены по чётным строкам (TILE чётный), а ужатие идёт
//! точными блоками 2×2 (`resample::halve`), поэтому пополосный проход даёт в
//! точности тот же растр, что ужатие уровня целиком: содержимое уровня k+1
//! не знает, какими порциями приезжал уровень k. Это закреплено тестом ниже.

use super::pyramid::{self, TILE};
use super::resample::halve;

/// Приёмник готовых тайлов: уровень, адрес в сетке, фактические размеры,
/// RGBA8. Ошибка приёмника останавливает проход.
pub type Emit<'a> = &'a mut dyn FnMut(u32, u32, u32, u32, u32, &[u8]) -> Result<(), String>;

pub struct Cascade {
    /// Полосы уровней от базового к вершине; индекс в векторе — не номер
    /// уровня: базовым может быть и не нулевой (JPEG декодируется сразу в
    /// масштаб запрошенного уровня).
    bands: Vec<Band>,
}

struct Band {
    /// Абсолютный номер уровня пирамиды.
    level: u32,
    width: u32,
    height: u32,
    /// Глобальная строка уровня, с которой начинается текущая полоса.
    top: u32,
    /// Заполненных строк в текущей полосе.
    filled: u32,
    buf: Vec<u8>,
}

impl Band {
    fn new(level: u32, width: u32, height: u32) -> Self {
        let rows = height.min(TILE);
        Self {
            level,
            width,
            height,
            top: 0,
            filled: 0,
            buf: vec![0; (width as usize) * (rows as usize) * 4],
        }
    }

    /// Строк в текущей полосе: у нижнего края короче.
    fn rows(&self) -> u32 {
        (self.height - self.top).min(TILE)
    }
}

impl Cascade {
    /// Полосы для уровней `base_level..`, вниз до вершины (большая сторона ≤
    /// тайла). Базовые размеры — размеры растра на `base_level`, и правило
    /// деления пополам то же, что в `pyramid::level_size`, — уровни каскада
    /// совпадают с уровнями пирамиды по построению.
    pub fn new(base_level: u32, base_w: u32, base_h: u32) -> Self {
        let mut bands = Vec::new();
        let (mut w, mut h) = (base_w, base_h);
        let mut level = base_level;
        loop {
            bands.push(Band::new(level, w, h));
            if w.max(h) <= TILE {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            level += 1;
        }
        Self { bands }
    }

    /// Группа полных строк базового уровня, сверху вниз, RGBA8 подряд.
    /// Высота группы любая — деление по границам полос здесь.
    pub fn push_rows(&mut self, rows: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        self.feed(0, rows, nrows, emit)
    }

    /// Конец источника: дожать неполные полосы всех уровней. Порядок сверху
    /// вниз обязателен — неполная полоса уровня k доносит строки в k+1.
    pub fn finish(mut self, emit: Emit) -> Result<(), String> {
        for i in 0..self.bands.len() {
            self.flush(i, emit)?;
        }
        Ok(())
    }

    fn feed(&mut self, i: usize, data: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        let width = self.bands[i].width as usize;
        let mut fed = 0u32;
        while fed < nrows {
            let band = &mut self.bands[i];
            let take = (band.rows() - band.filled).min(nrows - fed);
            // Полоса у нижнего края и взять больше нечего: источник отдал
            // строк больше, чем объявил. Ошибка, а не игнор: молча выброшенные
            // строки — это сдвиг всего, что ниже, а нулевой take здесь
            // зациклил бы проход.
            if take == 0 {
                return Err(format!(
                    "уровень {}: строк больше объявленной высоты {}",
                    band.level, band.height
                ));
            }
            let src = &data[(fed as usize) * width * 4..][..(take as usize) * width * 4];
            let at = (band.filled as usize) * width * 4;
            band.buf[at..at + src.len()].copy_from_slice(src);
            band.filled += take;
            fed += take;
            if band.filled == band.rows() {
                self.flush(i, emit)?;
            }
        }
        Ok(())
    }

    /// Нарезает заполненную часть полосы в тайлы и ужимает её в полосу
    /// следующего уровня.
    fn flush(&mut self, i: usize, emit: Emit) -> Result<(), String> {
        let (level, width, top, rows) = {
            let band = &self.bands[i];
            (band.level, band.width, band.top, band.filled)
        };
        if rows == 0 {
            return Ok(());
        }

        let ty = top / TILE;
        for tx in 0..pyramid::grid(width) {
            let tw = pyramid::tile_extent(tx, width);
            let mut tile = Vec::with_capacity((tw as usize) * (rows as usize) * 4);
            let buf = &self.bands[i].buf;
            for row in 0..rows as usize {
                let from = (row * (width as usize) + (tx * TILE) as usize) * 4;
                tile.extend_from_slice(&buf[from..from + (tw as usize) * 4]);
            }
            emit(level, tx, ty, tw, rows, &tile)?;
        }

        let half = if i + 1 < self.bands.len() {
            let band = &self.bands[i];
            let filled = &band.buf[..(width as usize) * (rows as usize) * 4];
            // Ширины уровней согласованы одной формулой деления пополам.
            debug_assert_eq!(width.div_ceil(2), self.bands[i + 1].width);
            Some((halve(filled, width, rows), rows.div_ceil(2)))
        } else {
            None
        };

        {
            let band = &mut self.bands[i];
            band.top += rows;
            band.filled = 0;
        }
        if let Some((half, hrows)) = half {
            self.feed(i + 1, &half, hrows, emit)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Детерминированный «шум»: содержимое не важно, важно несовпадение
    /// соседних пикселей.
    fn image(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        let mut x: u32 = 0x2545_F491;
        for _ in 0..w * h * 4 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((x >> 24) as u8);
        }
        v
    }

    /// Собирает уровень из пришедших тайлов; заодно проверяет, что тайлы
    /// не повторялись и их размеры совпали с арифметикой пирамиды.
    fn assemble(tiles: &HashMap<(u32, u32, u32), (u32, u32, Vec<u8>)>, level: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 4) as usize];
        for ty in 0..pyramid::grid(h) {
            for tx in 0..pyramid::grid(w) {
                let (tw, th, data) = tiles.get(&(level, tx, ty)).expect("тайл пришёл");
                assert_eq!((*tw, *th), (pyramid::tile_extent(tx, w), pyramid::tile_extent(ty, h)));
                for row in 0..*th {
                    let to = (((ty * TILE + row) * w + tx * TILE) * 4) as usize;
                    let from = ((row * tw) * 4) as usize;
                    out[to..to + (*tw as usize) * 4]
                        .copy_from_slice(&data[from..from + (*tw as usize) * 4]);
                }
            }
        }
        out
    }

    #[test]
    fn cascade_equals_whole_level_resample() {
        // Ширина и высота нарочно не кратны ни тайлу, ни двум.
        let (w, h) = (1301u32, 523u32);
        let src = image(w, h);

        let mut tiles: HashMap<(u32, u32, u32), (u32, u32, Vec<u8>)> = HashMap::new();
        let mut emit = |level: u32, tx: u32, ty: u32, tw: u32, th: u32, data: &[u8]| {
            let prev = tiles.insert((level, tx, ty), (tw, th, data.to_vec()));
            assert!(prev.is_none(), "тайл {}:{}:{} пришёл дважды", level, tx, ty);
            Ok(())
        };

        // Строки — порциями рваных размеров, как их отдают декодеры.
        let mut cascade = Cascade::new(0, w, h);
        let mut y = 0u32;
        for chunk in [1u32, 7, 500, 12, 3].iter().cycle() {
            if y >= h {
                break;
            }
            let take = (*chunk).min(h - y);
            let from = (y * w * 4) as usize;
            cascade.push_rows(&src[from..from + (take * w * 4) as usize], take, &mut emit).unwrap();
            y += take;
        }
        cascade.finish(&mut emit).unwrap();

        // Эталон — ужатие уровня целиком, тем же halve: пополосная сборка
        // не имеет права зависеть от порций.
        let levels = pyramid::level_count(w, h);
        let (mut ref_img, mut rw, mut rh) = (src.clone(), w, h);
        for level in 0..levels {
            assert_eq!(assemble(&tiles, level, rw, rh), ref_img, "уровень {}", level);
            ref_img = halve(&ref_img, rw, rh);
            (rw, rh) = (rw.div_ceil(2), rh.div_ceil(2));
        }
        // Тайлов сверх посчитанных уровней не приходило.
        assert!(tiles.keys().all(|(level, _, _)| *level < levels));
    }

    #[test]
    fn base_level_offsets_numbering() {
        // База не нулевая (так декодирует JPEG): номера уровней абсолютные.
        let (w, h) = (700u32, 40u32);
        let src = image(w, h);
        let mut seen = Vec::new();
        let mut emit = |level: u32, tx: u32, ty: u32, _tw: u32, _th: u32, _d: &[u8]| {
            seen.push((level, tx, ty));
            Ok(())
        };
        let mut cascade = Cascade::new(2, w, h);
        cascade.push_rows(&src, h, &mut emit).unwrap();
        cascade.finish(&mut emit).unwrap();
        // 700 → 350: два уровня, номера 2 и 3.
        assert_eq!(
            seen,
            vec![(2, 0, 0), (2, 1, 0), (3, 0, 0)],
        );
    }

    #[test]
    fn overfeed_is_an_error_not_a_hang() {
        // Битый файл: заголовок объявил 3 строки, поток отдал 4.
        let (w, h) = (8u32, 3u32);
        let src = image(w, h + 1);
        let mut emit = |_: u32, _: u32, _: u32, _: u32, _: u32, _: &[u8]| Ok(());
        let mut cascade = Cascade::new(0, w, h);
        let err = cascade.push_rows(&src, h + 1, &mut emit).unwrap_err();
        assert!(err.contains("больше объявленной высоты"), "{}", err);
    }

    #[test]
    fn error_from_sink_stops_pass() {
        let (w, h) = (600u32, 600u32);
        let src = image(w, h);
        let mut calls = 0;
        let mut emit = |_: u32, _: u32, _: u32, _: u32, _: u32, _: &[u8]| {
            calls += 1;
            Err("хватит".to_string())
        };
        let mut cascade = Cascade::new(0, w, h);
        let err = cascade.push_rows(&src, h, &mut emit).unwrap_err();
        assert_eq!(err, "хватит");
        assert_eq!(calls, 1);
    }
}
