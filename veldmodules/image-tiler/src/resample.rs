//! Ужатие RGBA8: два алгоритма под два разных контракта.
//!
//! [`halve`] — шаг пирамиды: пиксель уровня k+1 — это ровно блок 2×2 уровня
//! k (у краёв — обрезанный). Именно *блок*, а не пропорциональная доля:
//! на целочисленном соответствии координат стоит вся адресация пирамиды —
//! тайл (z+1, x, y) накрывает тайлы (z, 2x.., 2y..), и рисующий подставляет
//! грубый уровень под мелкий без пересчёта. Равномерное растяжение при
//! нечётной стороне сдвигало бы содержимое относительно этой сетки.
//!
//! [`resample`] — приведение к произвольному размеру с точными площадными
//! весами: обзорный регион TIFF → сетка уровня, кадр JPEG → размеры уровня.
//! Отношения там дробные (обзоры не обязаны быть половинами), и пиксель на
//! границе ячейки входит в обе с весом своей доли. Увеличение работает тем
//! же кодом, но по смыслу это размазывание — честная заглушка, когда
//! источник мельче цели.
//!
//! Цвет в обоих усредняется с весом альфы: у прозрачного пикселя цвета нет,
//! и поровну усреднённый он протекал бы чёрным в кромку данных (поля гранул —
//! прозрачный чёрный, см. ключевание nodata). Сплошь непрозрачный вход это
//! не меняет ни на байт: вес каждого слагаемого просто умножается на 255.

/// Шаг пирамиды: RGBA8 `w`×`h` → `ceil(w/2)`×`ceil(h/2)`, каждый целевой
/// пиксель — среднее своего блока 2×2 (у правого и нижнего краёв блок
/// обрезан). Округление к ближайшему; формула размеров та же, что в
/// `pyramid::level_size`, — уровни совпадают по построению.
pub fn halve(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    debug_assert_eq!(src.len(), (w as usize) * (h as usize) * 4);

    let (dw, dh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for dy in 0..dh {
        let y0 = dy * 2;
        let y1 = (y0 + 2).min(h);
        for dx in 0..dw {
            let x0 = dx * 2;
            let x1 = (x0 + 2).min(w);
            let mut rgb = [0u32; 3];
            let mut alpha = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let px = ((y as usize) * (w as usize) + (x as usize)) * 4;
                    let a = u32::from(src[px + 3]);
                    for c in 0..3 {
                        rgb[c] += u32::from(src[px + c]) * a;
                    }
                    alpha += a;
                }
            }
            let area = (y1 - y0) * (x1 - x0);
            let out_px = ((dy as usize) * (dw as usize) + (dx as usize)) * 4;
            for c in 0..3 {
                // Блок целиком прозрачен — цвета у него нет; ноль держит
                // выход детерминированным.
                out[out_px + c] = if alpha == 0 { 0 } else { ((rgb[c] + alpha / 2) / alpha) as u8 };
            }
            out[out_px + 3] = ((alpha + area / 2) / area) as u8;
        }
    }
    out
}

/// `src` — RGBA8 размера `sw`×`sh`; результат — RGBA8 размера `dw`×`dh`.
/// Пустые размеры дают пустой буфер: звать так — ошибка вызывающего,
/// но паниковать в конвейере из-за неё не за чем.
pub fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Vec::new();
    }
    debug_assert_eq!(src.len(), (sw as usize) * (sh as usize) * 4);

    // Быстрый путь тождества: каскад зовёт ресемпл безусловно, а полоса
    // вершины уже нужного размера.
    if sw == dw && sh == dh {
        return src.to_vec();
    }

    let x_spans = spans(sw, dw);
    let y_spans = spans(sh, dh);

    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for (dy, (y0, y1)) in y_spans.iter().enumerate() {
        for (dx, (x0, x1)) in x_spans.iter().enumerate() {
            let mut rgb = [0f64; 3];
            // Σ альфа·вес: числитель альфы и он же — знаменатель цвета.
            let mut alpha = 0f64;
            let mut area = 0f64;
            let mut sy = y0.floor() as u32;
            while (sy as f64) < *y1 {
                let wy = overlap(sy, *y0, *y1);
                let row = (sy as usize) * (sw as usize) * 4;
                let mut sx = x0.floor() as u32;
                while (sx as f64) < *x1 {
                    let wx = overlap(sx, *x0, *x1);
                    let px = row + (sx as usize) * 4;
                    let w = wx * wy;
                    let aw = f64::from(src[px + 3]) * w;
                    for c in 0..3 {
                        rgb[c] += f64::from(src[px + c]) * aw;
                    }
                    alpha += aw;
                    area += w;
                    sx += 1;
                }
                sy += 1;
            }
            let out_px = (dy * (dw as usize) + dx) * 4;
            for c in 0..3 {
                // Ячейка целиком прозрачна — цвета у неё нет; ноль держит
                // выход детерминированным.
                out[out_px + c] = if alpha == 0.0 {
                    0
                } else {
                    (rgb[c] / alpha).round().clamp(0.0, 255.0) as u8
                };
            }
            // Площадь не бывает нулём: спаны не пусты по построению.
            out[out_px + 3] = (alpha / area).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Границы исходного диапазона для каждого целевого индекса, в исходных
/// пикселях: ячейка d накрывает [d·s/d_total, (d+1)·s/d_total).
fn spans(src: u32, dst: u32) -> Vec<(f64, f64)> {
    let scale = f64::from(src) / f64::from(dst);
    (0..dst)
        .map(|d| {
            let a = f64::from(d) * scale;
            let b = (f64::from(d) + 1.0) * scale;
            // Правый край клампится источником: плавающая арифметика может
            // дать s + ε, а пикселя за краем нет.
            (a, b.min(f64::from(src)))
        })
        .collect()
}

/// Доля пикселя `s` внутри диапазона [a, b).
fn overlap(s: u32, a: f64, b: f64) -> f64 {
    let lo = f64::from(s).max(a);
    let hi = f64::from(s + 1).min(b);
    (hi - lo).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, value: u8) -> Vec<u8> {
        vec![value; (w * h * 4) as usize]
    }

    #[test]
    fn uniform_stays_uniform() {
        // Любое отношение сторон: среднее константы — та же константа.
        for (sw, sh, dw, dh) in [(7, 5, 3, 2), (10, 10, 5, 5), (3, 3, 4, 4), (513, 2, 257, 1)] {
            let out = resample(&solid(sw, sh, 137), sw, sh, dw, dh);
            assert!(out.iter().all(|&v| v == 137), "{}x{} → {}x{}", sw, sh, dw, dh);
        }
    }

    #[test]
    fn exact_two_to_one_is_block_average() {
        // 4×2 → 2×1: каждая ячейка — среднее своего блока 2×2.
        let mut src = Vec::new();
        for v in [10u8, 20, 30, 40, 50, 60, 70, 80] {
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let out = resample(&src, 4, 2, 2, 1);
        // Блок левый: 10,20,50,60 → 35; правый: 30,40,70,80 → 55.
        assert_eq!(&out[0..4], &[35, 35, 35, 255]);
        assert_eq!(&out[4..8], &[55, 55, 55, 255]);
    }

    #[test]
    fn fractional_ratio_preserves_mean() {
        // 3 → 2 по ширине: средний пиксель делится между ячейками пополам,
        // и среднее по картинке сохраняется.
        let mut src = Vec::new();
        for v in [0u8, 90, 180] {
            src.extend_from_slice(&[v, 0, 0, 255]);
        }
        let out = resample(&src, 3, 1, 2, 1);
        // Ячейка 0: (0·1 + 90·0.5)/1.5 = 30; ячейка 1: (90·0.5 + 180·1)/1.5 = 150.
        assert_eq!(out[0], 30);
        assert_eq!(out[4], 150);
    }

    #[test]
    fn identity_is_copy() {
        let src: Vec<u8> = (0..4 * 6 * 4).map(|i| (i % 251) as u8).collect();
        assert_eq!(resample(&src, 4, 6, 4, 6), src);
    }

    #[test]
    fn halve_averages_integer_blocks() {
        // 3×3 → 2×2: полный блок, обрезанные по краю и угловой из одного.
        let mut src = Vec::new();
        for v in [10u8, 20, 30, 40, 50, 60, 70, 80, 90] {
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let out = halve(&src, 3, 3);
        // Блоки: (10,20,40,50)=30; (30,60)=45; (70,80)=75; (90)=90.
        assert_eq!(&out[0..4], &[30, 30, 30, 255]);
        assert_eq!(&out[4..8], &[45, 45, 45, 255]);
        assert_eq!(&out[8..12], &[75, 75, 75, 255]);
        assert_eq!(&out[12..16], &[90, 90, 90, 255]);
    }

    #[test]
    fn halve_rounds_to_nearest() {
        // (0 + 1) / 2 = 0.5 → 1: округление, а не отбрасывание.
        let src = [0, 0, 0, 255, 1, 1, 1, 255];
        assert_eq!(&halve(&src, 2, 1)[..3], &[1, 1, 1]);
    }

    #[test]
    fn transparent_does_not_bleed_into_colour() {
        // Блок 2×2: два белых и два прозрачных чёрных (поле гранулы). Цвет
        // остаётся белым, полупрозрачность несёт одна альфа.
        let src = [255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0];
        assert_eq!(halve(&src, 2, 2), vec![255, 255, 255, 128]);
        assert_eq!(resample(&src, 2, 2, 1, 1), vec![255, 255, 255, 128]);
    }

    #[test]
    fn fully_transparent_block_stays_empty() {
        let src = [9, 9, 9, 0, 9, 9, 9, 0];
        assert_eq!(halve(&src, 2, 1), vec![0, 0, 0, 0]);
        assert_eq!(resample(&src, 2, 1, 1, 1), vec![0, 0, 0, 0]);
    }
}
