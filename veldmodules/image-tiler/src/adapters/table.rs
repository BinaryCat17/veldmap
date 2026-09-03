//! Таблица уровней источника: как обслуживается каждый уровень, чего он стоит
//! и влезает ли в память. Одна на описание и производство: она же строками
//! уезжает на провод (`Described.levels`), и по ней же `adapters::produce`
//! выбирает рукав — правило записано один раз.
//!
//! Строки считаются по заголовку, без пикселей: у TIFF по сетке чанков
//! (`Grid::footprint`), у остальных по кадру и правилам их декодеров. Пик —
//! `budget::Peak`, столбец «влезает» — его сверка со свободным вместе с
//! потолком пути, если у пути он свой: `FULL_DECODE_BUDGET` у кадра целиком.

use super::super::budget::Peak;
use super::super::cascade;
use super::super::pyramid;
use super::grid::Overview;
use super::{frame_fits, jpeg, netcdf, Info, Kind, Tie};

/// Как уровень обслуживается.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    /// Точечно: читаются только чанки под заказанными тайлами.
    Pointwise,
    /// Проходом, который начинается на уровне `from` и строит каскадом его и
    /// все грубее. Ноль — вся пирамида одним проходом; сам уровень — декодер
    /// отдаёт кадр сразу в его масштаб, и за подробным приходят новым проходом.
    Pass { from: u32 },
}

/// Строка таблицы — один уровень пирамиды.
#[derive(Clone, Debug)]
pub struct Level {
    pub serve: Serve,
    /// Чего стоит чтение: пикселей источника, распакованных ради одного тайла
    /// (точечно) либо за весь проход (растр целиком).
    pub pixels: u64,
    /// Пик памяти работы на этом уровне — именованными слагаемыми.
    pub peak: Peak,
    /// Влезает ли работа в память: пик против свободного вместе с потолком
    /// пути, если у пути он свой.
    pub fits: bool,
}

impl Info {
    /// Таблица уровней, от нулевого к вершине.
    pub fn levels(&self) -> Vec<Level> {
        let base = (self.width, self.height);
        let whole = u64::from(self.width) * u64::from(self.height);
        let pass = |peak: Peak, fits: bool| Level { serve: Serve::Pass { from: 0 }, pixels: whole, fits, peak };
        (0..pyramid::level_count(self.width, self.height))
            .map(|level| {
                let lw = pyramid::level_size(self.width, level);
                let lh = pyramid::level_size(self.height, level);
                // Сетка чанков — точечно, где окно и правда окно, иначе проходом.
                let chunked = |grid: &super::grid::Grid| match grid.footprint(base, level) {
                    Some(footprint) => {
                        let peak = grid.direct_peak(base, level).expect("уровень точечный");
                        Level { serve: Serve::Pointwise, pixels: footprint.held, fits: peak.fits(), peak }
                    }
                    None => {
                        let peak = grid.pass_peak(base);
                        pass(peak.clone(), peak.fits())
                    }
                };
                match &self.kind {
                    Kind::Tiff(layout) => chunked(&layout.grid),
                    Kind::Jp2(layout) => chunked(&layout.grid),
                    // Потоковый PNG: строка и полосы каскада.
                    Kind::Png { interlaced: false } => {
                        let peak = Peak::new()
                            .with("строка", u64::from(self.width) * 4)
                            .with("каскад", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), peak.fits())
                    }
                    // Кадр целиком: декодированный и он же в RGBA.
                    Kind::Png { interlaced: true } | Kind::Full(_) => {
                        let peak = Peak::new()
                            .with("кадр", whole * 4 * 2)
                            .with("каскад", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), frame_fits(self.width, self.height) && peak.fits())
                    }
                    // Кадр в масштабе декодера, приведённый к сетке уровня.
                    Kind::Jpeg => {
                        let (dw, dh) = jpeg::decoded_size(self.width, self.height, level);
                        let decoded = u64::from(dw) * u64::from(dh);
                        let peak = Peak::new()
                            .with("сырой кадр", decoded * 3)
                            .with("кадр", decoded * 4)
                            .with("уровень", u64::from(lw) * u64::from(lh) * 4)
                            .with("каскад", cascade::bytes(lw, lh));
                        Level {
                            serve: Serve::Pass { from: level },
                            pixels: whole,
                            fits: frame_fits(dw, dh) && peak.fits(),
                            peak,
                        }
                    }
                    // Плоскость уже осела в разборе; проход разворачивает её полосами.
                    Kind::Netcdf(source) => {
                        let peak = Peak::new()
                            .with("плоскость", source.plane_bytes())
                            .with("каскад", cascade::bytes(self.width, self.height))
                            .with("полоса", netcdf::strip_bytes(self.width, self.height));
                        pass(peak.clone(), peak.fits())
                    }
                }
            })
            .collect()
    }

    /// Строка уровня; `None` — уровня у пирамиды нет.
    pub fn level(&self, level: u32) -> Option<Level> {
        self.levels().into_iter().nth(level as usize)
    }

    /// Сколько уровней от нулевого читаются точечно — длина точечного начала
    /// таблицы.
    pub fn windowed(&self) -> u32 {
        self.levels().iter().take_while(|row| row.serve == Serve::Pointwise).count() as u32
    }

    /// Предел детали: первый уровень, который влезает. Не влезает ни один —
    /// вершина; такой источник описание не отдаёт (`adapters::checked`), и
    /// ответ здесь — на случай таблицы, посчитанной мимо описания. Потребитель
    /// считает то же по строкам провода (`tiles::Meta::finest`).
    pub fn finest(&self) -> u32 {
        let rows = self.levels();
        let top = rows.len().saturating_sub(1) as u32;
        rows.iter().position(|row| row.fits).map_or(top, |at| at as u32)
    }

    /// Сколько памяти держит сам разбор, пока лежит в memo: узлы привязки,
    /// сетка копий, у NetCDF — осевшая плоскость. Слагаемое пика соседней
    /// работы (`module::State::neighbour_footprint`).
    pub fn footprint(&self) -> u64 {
        let ties = (self.ties.len() * std::mem::size_of::<Tie>()) as u64;
        let kind = match &self.kind {
            Kind::Tiff(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Jp2(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Netcdf(source) => source.plane_bytes(),
            Kind::Png { .. } | Kind::Jpeg | Kind::Full(_) => 0,
        };
        ties + kind
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::budget;
    use super::super::super::pyramid::TILE;
    use super::super::codec::Format;
    use super::super::jp2;
    use super::super::tiff::Layout;
    use super::*;

    /// TCI гранулы Sentinel-2 как JPEG 2000: 10980², тайлы 1024², пять
    /// разрешений (ADR 0002).
    fn tci() -> Info {
        let codestream = jp2::Codestream {
            canvas: (10980, 10980),
            origin: (0, 0),
            tile: (1024, 1024),
            tile_origin: (0, 0),
            components: 3,
            precision: 8,
            resolutions: 5,
            progression: 0,
            layers: 1,
            tlm_parts: Some(121),
            plt_first: Some(true),
        };
        Info::plain(10980, 10980, Kind::Jp2(jp2::Layout::of(&codestream, 130 << 20, Format::Jp2, Some(0.0), None)))
    }

    /// Байт на пиксель RGB8 — таблице геометрии глубина сэмпла безразлична.
    const RGB8: u32 = 3;

    /// Полосный TIFF без копий, как пишет GDAL гранулу Sentinel-1 GRD без
    /// внутренних тайлов: полоса в одну строку во всю ширину.
    fn stripped(width: u32, height: u32) -> Info {
        Info::plain(width, height, Kind::Tiff(Layout::of(false, (width, 1), Vec::new(), RGB8)))
    }

    /// Тайловый TIFF с полной цепочкой копий-половин.
    fn cog(width: u32, height: u32) -> Info {
        let overviews = (1..pyramid::level_count(width, height))
            .map(|level| Overview {
                image: level as usize,
                width: pyramid::level_size(width, level),
                height: pyramid::level_size(height, level),
                chunk: (TILE, TILE),
            })
            .collect();
        Info::plain(width, height, Kind::Tiff(Layout::of(true, (TILE, TILE), overviews, RGB8)))
    }

    /// На настоящих размерах всё, что читается точечно или проходом, влезает
    /// на каждом уровне, и предел детали у таких — родное разрешение; а
    /// декодер кадра целиком влезает не везде, и таблица это говорит.
    ///
    /// Размеры настоящие: Sentinel-1 GRD полосный и он же COG, Landsat 7,
    /// квиклук, TCI Sentinel-2 как JPEG 2000 — тайлами кодстрима.
    #[test]
    fn настоящие_размеры_влезают_по_таблице() {
        let sources = [
            stripped(25309, 17408),
            cog(25309, 17408),
            cog(8271, 8391),
            Info::plain(2422, 1940, Kind::Png { interlaced: false }),
            tci(),
        ];
        for info in &sources {
            let rows = info.levels();
            assert_eq!(rows.len() as u32, pyramid::level_count(info.width, info.height));
            for (level, row) in rows.iter().enumerate() {
                assert!(
                    row.fits && row.peak.total() <= budget::free(),
                    "{}×{} уровень {}: {}", info.width, info.height, level, row.peak.note()
                );
            }
            assert_eq!(info.finest(), 0, "{}×{}", info.width, info.height);
        }

        // Каждый уровень TCI — точечно из своей копии либо из самой мелкой
        // (вершине копии не достаётся: разрешений пять, уровней шесть).
        assert!(tci().levels().iter().all(|row| row.serve == Serve::Pointwise));
    }

    /// Столбец обслуживания у четырёх родов источника: точечно везде (COG), на
    /// части уровней (полосный — точечное начало ровно по длине окна), проходом
    /// с нулевого на всех (PNG), проходом со своего уровня на каждом (JPEG).
    #[test]
    fn столбец_обслуживания_различает_четыре_рода_источника() {
        let exact = cog(4 * TILE, 4 * TILE);
        assert_eq!(exact.windowed(), exact.levels().len() as u32);

        let strips = stripped(25309, 17408);
        let rows = strips.levels();
        let prefix = rows.iter().take_while(|row| row.serve == Serve::Pointwise).count() as u32;
        assert!(prefix > 0 && prefix < rows.len() as u32, "окно на части уровней: {prefix}");
        assert_eq!(strips.windowed(), prefix);
        assert!(rows[prefix as usize..].iter().all(|row| row.serve == Serve::Pass { from: 0 }));

        let png = Info::plain(2048, 2048, Kind::Png { interlaced: false });
        assert_eq!(png.windowed(), 0);
        assert!(png.levels().iter().all(|row| row.serve == Serve::Pass { from: 0 }));

        let jpeg = Info::plain(2048, 2048, Kind::Jpeg);
        assert!(jpeg.levels().iter().enumerate().all(|(at, row)| row.serve == Serve::Pass { from: at as u32 }));
    }

    /// Предел детали — первый влезающий уровень: у JPEG во весь TCI — первый,
    /// чей кадр в масштабе декодера влезает в потолок кадра; у точечных COG и
    /// TCI как JPEG 2000 — нулевой.
    #[test]
    fn предел_детали_это_первый_влезающий_уровень() {
        assert_eq!(tci().finest(), 0);

        let jpeg = Info::plain(10980, 10980, Kind::Jpeg);
        let finest = jpeg.finest();
        assert!(finest > 0, "кадр 10980² в RGBA больше потолка кадра");
        assert!(!jpeg.level(finest - 1).unwrap().fits && jpeg.level(finest).unwrap().fits);

        assert_eq!(cog(25309, 17408).finest(), 0);
    }

    /// Вырожденная сетка — чанк мельче [`grid::MIN_CHUNK_PIXELS`] — читается
    /// только проходом: так лежит квиклук PVI Sentinel-2 (8×8), и точечное
    /// чтение платило бы за вызов декодера на каждые 64 пикселя.
    #[test]
    fn вырожденная_сетка_идёт_проходом() {
        let pvi = Info::plain(343, 343, Kind::Tiff(Layout::of(true, (8, 8), Vec::new(), RGB8)));
        assert!(pvi.levels().iter().all(|row| row.serve == Serve::Pass { from: 0 }));

        // А полоса в одну строку во всю ширину сеткой не вырождена: она
        // крупнее порога, и нулевой уровень читается точечно.
        assert_eq!(stripped(25309, 17408).level(0).unwrap().serve, Serve::Pointwise);
    }

    /// Разбор соседа весит столько, сколько держит: у плоскости NetCDF —
    /// плоскость, у заголовков — узлы привязки и копии.
    #[test]
    fn след_разбора_считается_по_тому_что_он_держит() {
        assert_eq!(Info::plain(64, 64, Kind::Jpeg).footprint(), 0);
        let mut tiff = cog(4096, 4096);
        tiff.ties = (0..441).map(|_| Tie { px: 0.0, py: 0.0, lat: 0.0, lon: 0.0 }).collect();
        assert!(tiff.footprint() >= 441 * 32, "узлы привязки не посчитаны");
        assert_eq!(Info::heavy(1500, 1202).footprint(), 0, "пустая плоскость — ноль");
    }
}
