//! Коробочный даунсемпл исходника в превью заданного бокса.
//!
//! Приёмник пикселей: каждый пиксель источника добавляется ровно один раз в
//! свою ячейку превью, а порядок поступления не важен. Поэтому один и тот же
//! приёмник обслуживает и построчный PNG, и стрипы/тайлы TIFF, и целый буфер
//! от декодера, который потоковым путём не умеет.
//!
//! Память — только само превью: от размера исходника она не зависит, ради
//! чего всё и затевалось.

pub struct Downsampler {
    src_w: u32,
    src_h: u32,
    /// Во сколько раз ужимаем (целое: ячейка превью = блок scale×scale).
    scale: u32,
    out_w: u32,
    out_h: u32,
    /// Суммы каналов по ячейкам, out_w * out_h * 4.
    acc: Vec<u32>,
}

impl Downsampler {
    /// Бокс (`max_w`×`max_h`) — верхняя граница превью; пропорции сохраняются.
    pub fn new(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> Self {
        let scale = div_ceil(src_w, max_w).max(div_ceil(src_h, max_h)).max(1);
        let out_w = div_ceil(src_w, scale).max(1);
        let out_h = div_ceil(src_h, scale).max(1);
        Self { src_w, src_h, scale, out_w, out_h, acc: vec![0; (out_w * out_h * 4) as usize] }
    }

    pub fn out_size(&self) -> (u32, u32) {
        (self.out_w, self.out_h)
    }

    /// RGBA8-пиксели прямоугольника источника, построчно слева направо.
    /// Лишние строки/столбцы за границей источника игнорируются: у краевых
    /// чанков TIFF данные бывают шире полезной части.
    pub fn add_rect(&mut self, x0: u32, y0: u32, w: u32, h: u32, rgba: &[u8]) {
        for row in 0..h {
            let y = y0 + row;
            if y >= self.src_h { break; }
            let cell_y = y / self.scale;
            let row_start = (row * w * 4) as usize;
            for col in 0..w {
                let x = x0 + col;
                if x >= self.src_w { break; }
                let px = row_start + (col * 4) as usize;
                let Some(px) = rgba.get(px..px + 4) else { return };
                let cell = ((cell_y * self.out_w + x / self.scale) * 4) as usize;
                for c in 0..4 {
                    self.acc[cell + c] += u32::from(px[c]);
                }
            }
        }
    }

    /// Готовое превью RGBA8. Делитель ячейки считается по её реальной площади:
    /// у правого и нижнего краёв блок обрезан размерами источника.
    pub fn finish(self) -> Vec<u8> {
        let mut out = vec![0u8; (self.out_w * self.out_h * 4) as usize];
        for cy in 0..self.out_h {
            let rows = cell_extent(cy, self.scale, self.src_h);
            for cx in 0..self.out_w {
                let area = rows * cell_extent(cx, self.scale, self.src_w);
                let cell = ((cy * self.out_w + cx) * 4) as usize;
                for c in 0..4 {
                    out[cell + c] = (self.acc[cell + c] / area.max(1)) as u8;
                }
            }
        }
        out
    }
}

/// Сколько пикселей источника реально попадает в ячейку по одной оси.
fn cell_extent(cell: u32, scale: u32, src: u32) -> u32 {
    let start = cell * scale;
    src.min(start + scale).saturating_sub(start)
}

fn div_ceil(a: u32, b: u32) -> u32 {
    if b == 0 { return 1; }
    a.div_ceil(b)
}
