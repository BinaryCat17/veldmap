//! Каскад полос: все уровни пирамиды за один проход по источнику.
//!
//! Вход — полнокровные группы строк базового уровня, сверху вниз; выход —
//! готовые тайлы каждого уровня через колбэк, в момент, когда полоса уровня
//! дозаполнилась. Полоса — это `TILE` строк уровня: заполнилась → нарезана в
//! тайлы и ужата вдвое в полосу следующего уровня. Память — по одной полосе на
//! уровень, геометрическая прогрессия от ширины; от высоты источника она
//! зависит только числом уровней. Точную цену называет [`bytes`], и спрашивать
//! её надо там, а не выводить из этой строки: у зовущего сверх неё живёт ещё и
//! полоса, которую он каскаду отдаёт.
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
    /// Временное дожатия: живо сейчас и было в пике. Этим и сверяется
    /// предсказание [`flush_bytes`] — не со второй формулой рядом, а с тем,
    /// что проход выделил на самом деле.
    #[cfg(test)]
    spent: Spent,
}

#[cfg(test)]
#[derive(Default)]
struct Spent {
    live: u64,
    peak: u64,
}

#[cfg(test)]
impl Spent {
    fn take(&mut self, bytes: u64) {
        self.live += bytes;
        self.peak = self.peak.max(self.live);
    }

    fn give(&mut self, bytes: u64) {
        self.live -= bytes;
    }
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

/// Во что каскад обойдётся по памяти инстанса — до того, как его завели.
///
/// Спрашивается это теми, кто решает, браться ли за источник вообще: цена
/// каскада складывается с ценой самого чтения, а лимит инстанса один на обоих.
/// Считается здесь, а не у спрашивающего, по той же причине, по какой каскад
/// сам знает свои уровни: вывод «на глаз» из ширины разъезжается с кодом молча,
/// и разъехавшийся он не мешает ни сборке, ни тестам.
///
/// Слагаемых два.
///
/// **Полосы уровней живут весь проход.** Их ровно столько, сколько уровней, и
/// каждая — `ширина уровня × min(высота уровня, TILE) × 4`. Ширины уровней
/// сходятся геометрически, поэтому сумма примерно вдвое больше базовой полосы,
/// а от высоты источника не зависит вовсе.
///
/// **Дожатие полосы рекурсивно, и его временные буферы живут разом.** `flush`
/// уровня режет тайл (не больше `TILE × TILE × 4`), ужимает полосу вдвое и с
/// этим ужатым идёт в `flush` следующего уровня — где всё повторяется, а
/// ужатое предыдущего ещё живо. Считается поэтому сумма по всем уровням, а не
/// худший из них.
pub fn bytes(base_w: u32, base_h: u32) -> u64 {
    bands_bytes(base_w, base_h) + flush_bytes(base_w, base_h)
}

/// Полосы уровней: то, что живёт весь проход. Считается тем же обходом, каким
/// они строятся, и сверено с постройкой тестом.
fn bands_bytes(base_w: u32, base_h: u32) -> u64 {
    levels(base_w, base_h).map(|(w, rows)| u64::from(w) * rows * 4).sum()
}

/// Временное дожатия: вырезанный тайл и ужатая вдвое полоса.
///
/// Слагаемые считаются по-разному, потому что живут по-разному. **Ужатое
/// складывается по уровням**: `flush` уровня уходит в `flush` следующего, не
/// отпустив своего, — и на глубине k живы все ужатые выше. **Тайл берётся один
/// самый большой**: он объявлен в теле цикла по тайлам, гибнет на каждом витке
/// и к моменту ужатия мёртв, так что двух тайлов разом не бывает нигде.
fn flush_bytes(base_w: u32, base_h: u32) -> u64 {
    let halves: u64 = levels(base_w, base_h)
        .map(|(w, rows)| u64::from(w.div_ceil(2)) * rows.div_ceil(2) * 4)
        .sum();
    let tile = levels(base_w, base_h)
        .map(|(w, rows)| u64::from(w.min(TILE)) * rows * 4)
        .max()
        .unwrap_or(0);
    halves + tile
}

/// Уровни каскада как пары «ширина, строк в полосе» — тем же делением пополам,
/// каким их строит [`Cascade::new`].
fn levels(base_w: u32, base_h: u32) -> impl Iterator<Item = (u32, u64)> {
    let (mut w, mut h) = (base_w, base_h);
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let item = (w, u64::from(h.min(TILE)));
        done = w.max(h) <= TILE;
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        Some(item)
    })
}

impl Cascade {
    /// Сколько памяти держат полосы прямо сейчас — для проверки, что
    /// предсказание [`bytes`] не разъехалось с постройкой.
    #[cfg(test)]
    fn held(&self) -> u64 {
        self.bands.iter().map(|band| band.buf.len() as u64).sum()
    }

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
        Self { bands, #[cfg(test)] spent: Spent::default() }
    }

    /// Группа полных строк базового уровня, сверху вниз, RGBA8 подряд.
    /// Высота группы любая — деление по границам полос здесь.
    pub fn push_rows(&mut self, rows: &[u8], nrows: u32, emit: Emit) -> Result<(), String> {
        self.feed(0, rows, nrows, emit)
    }

    /// Конец источника: дожать неполные полосы всех уровней. Порядок сверху
    /// вниз обязателен — неполная полоса уровня k доносит строки в k+1.
    pub fn finish(mut self, emit: Emit) -> Result<(), String> {
        self.drain(emit)
    }

    /// Тело [`Self::finish`] по ссылке — чтобы после дожатия можно было ещё
    /// спросить каскад, во что оно обошлось.
    fn drain(&mut self, emit: Emit) -> Result<(), String> {
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
            let taken = u64::from(tw) * u64::from(rows) * 4;
            #[cfg(test)]
            self.spent.take(taken);
            let mut tile = Vec::with_capacity(taken as usize);
            let buf = &self.bands[i].buf;
            for row in 0..rows as usize {
                let from = (row * (width as usize) + (tx * TILE) as usize) * 4;
                tile.extend_from_slice(&buf[from..from + (tw as usize) * 4]);
            }
            emit(level, tx, ty, tw, rows, &tile)?;
            drop(tile);
            #[cfg(test)]
            self.spent.give(taken);
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
            #[cfg(test)]
            let taken = half.len() as u64;
            #[cfg(test)]
            self.spent.take(taken);
            let outcome = self.feed(i + 1, &half, hrows, emit);
            drop(half);
            #[cfg(test)]
            self.spent.give(taken);
            outcome?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Предсказание цены полос сходится с постройкой. Сверяется именно с
    /// `Cascade::new`, а не с формулой рядом: две формулы, написанные по одному
    /// правилу, разъедутся молча, а постройка — это то, за что платят.
    #[test]
    fn цена_полос_сходится_с_постройкой() {
        for (w, h) in [(1, 1), (512, 512), (513, 300), (4865, 4091), (65536, 2), (3, 65536)] {
            assert_eq!(
                Cascade::new(0, w, h).held(),
                bands_bytes(w, h),
                "растр {}×{}: предсказание полос разошлось с постройкой",
                w, h
            );
        }
    }

    /// Базовый уровень цену не меняет: полосы считаются от размеров растра на
    /// этом уровне, а номер уровня в их память не входит.
    #[test]
    fn цена_не_зависит_от_номера_базового_уровня() {
        assert_eq!(Cascade::new(0, 2000, 1500).held(), Cascade::new(3, 2000, 1500).held());
    }

    /// Цену задаёт ширина. Высота входит в неё только числом уровней, то есть
    /// логарифмически: впятеро более высокий источник дороже на проценты, тогда
    /// как впятеро более широкий — в разы. На этом стои́т весь потоковый проход:
    /// источник любой высоты идёт полосами через почти постоянную память.
    #[test]
    fn цену_задаёт_ширина_а_высота_входит_логарифмом() {
        let wide = bytes(20_000, 1_000);
        let tall = bytes(1_000, 20_000);
        assert!(wide > tall * 4, "широкий {} против высокого {}", wide, tall);

        let (low, high) = (bytes(1_000, 20_000), bytes(1_000, 100_000));
        assert!(high > low, "лишние уровни чего-то да стоят");
        assert!(
            high - low < low / 20,
            "пятикратная высота подняла цену с {} до {} — это не логарифм",
            low, high
        );
    }

    /// Предсказание временного сходится с тем, что проход потратил на самом
    /// деле. Полосы сверены с постройкой отдельно; это второе слагаемое
    /// [`bytes`], и сверять его не с чем, кроме прохода: две формулы,
    /// написанные по одному правилу, расходятся молча.
    ///
    /// Кормится каскад рваными порциями нарочно — полосы уровней
    /// дозаполняются вразнобой, и рекурсия `flush → feed → flush` проходится
    /// вся, до самой вершины.
    #[test]
    fn предсказание_временного_сходится_с_проходом() {
        for (w, h) in [(1301u32, 523u32), (2000, 1500), (700, 3000), (5000, 517)] {
            let src = image(w, h);
            let mut emit = |_: u32, _: u32, _: u32, _: u32, _: u32, _: &[u8]| Ok(());
            let mut cascade = Cascade::new(0, w, h);

            let mut fed = 0u32;
            for take in [1u32, 7, 500, 12, 3].iter().cycle() {
                if fed >= h {
                    break;
                }
                let rows = (*take).min(h - fed);
                let from = (fed as usize) * (w as usize) * 4;
                let slice = &src[from..from + (rows as usize) * (w as usize) * 4];
                cascade.push_rows(slice, rows, &mut emit).expect("полоса принята");
                fed += rows;
            }
            cascade.drain(&mut emit).expect("хвост дожат");

            let spent = cascade.spent.peak;
            let predicted = flush_bytes(w, h);
            assert!(
                spent <= predicted,
                "растр {}×{}: проход потратил {} при обещанных {}",
                w, h, spent, predicted
            );
            // И не вдвое меньше: завышенное предсказание — это отказ читаемому
            // источнику, такой же промах, как заниженное.
            assert!(
                spent * 2 > predicted,
                "растр {}×{}: обещано {}, потрачено {} — предсказание завышено вдвое",
                w, h, predicted, spent
            );
        }
    }

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
