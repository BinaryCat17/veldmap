//! JPEG 2000 (JP2/J2C): квиклуки и гранулы Sentinel-2 — чанками драйвера.
//! Чанк — тайл кодстрима на факторе разрешения: декодирует его openjp2 за
//! unsafe-островом (`codec.rs`), а какие чанки читать и когда идти проходом,
//! решает общий драйвер сетки чанков (`grid.rs`) по раскладке из главного
//! заголовка ([`Layout`]). Описание кодека не запускает: размеры, сетку тайлов
//! и число разрешений читает свой разбор головы файла ([`codestream`]),
//! привязку — коробка GMLJP2 ([`gml_placement`]). Тайл кодеку подаётся
//! выдержкой по индексу TLM (`excerpt.rs`), а без TLM — всем файлом, по
//! которому кодек ходит сам. Что решено и почему —
//! `docs/decisions/0002-jpeg2000-decoder.md` и `0004-jp2-excerpt.md`.

use std::cell::{Cell, RefCell};
use std::io::{Read, Seek};
use std::rc::Rc;

use super::super::cascade::Emit;
use super::super::pyramid;
use super::codec::{Decoder, Format, Header, Tile};
use super::excerpt;
use super::grid::{self, Chunked, Grid, Overview};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples};
use super::{Info, Kind, Metered, Placement};

/// Сколько головы файла хватает заголовку: сигнатурные боксы, ftyp, jp2h
/// лежат первыми, а у сырого кодстрима SIZ — сразу за SOC.
const HEAD: usize = 64 * 1024;

/// Байт на отсчёт у декодера: плоскости `i32` у образа и столько же у тайлового
/// кодера внутри — счёт памяти чанка ведётся по ним, а не по разрядности файла.
const SAMPLE_BYTES: u32 = 8;

/// Раскладка файла для драйвера: сетка тайлов кодстрима с копиями по
/// разрешениям — и то, чего драйверу знать не нужно, а чанку нужно.
pub struct Layout {
    pub grid: Grid,
    /// Тайлов кодстрима по горизонтали и вертикали — одна сетка на всех
    /// факторах: индекс тайла у кодека от разрешения не зависит.
    pub tiles: (u32, u32),
    /// Длина файла: кодек считает по ней остаток потока.
    pub len: u64,
    format: Format,
    /// Разрядность отсчёта: до байта растяг не нужен, и решается это без
    /// кодека.
    precision: u8,
    /// Ноль — «нет данных»: у гранулы Sentinel-2 годные значения 1..255, и
    /// поля снимка приезжают нулём. Ставится только файлу с привязкой GMLJP2:
    /// у произвольного JP2 чёрному никто не запрещал быть цветом.
    nodata: Option<f32>,
    /// Растяг показа отсчётов шире байта — один на файл, иначе соседние тайлы
    /// разошлись бы яркостью (см. `tiff::Layout`).
    stretch: RefCell<Option<Mapping>>,
    /// Индекс тайл-партов по TLM; без него тайл ищет сам кодек, обходя SOT.
    index: Option<excerpt::Index>,
}

impl Layout {
    /// Копия на факторе `f` есть, пока тайл делится на `2^f` без остатка: так
    /// сетка тайлов на уменьшенной решётке остаётся равномерной, и она же
    /// — сетка чанков копии. Дальше уровни берут самую мелкую копию по общему
    /// правилу драйвера.
    pub fn of(
        codestream: &Codestream,
        len: u64,
        format: Format,
        nodata: Option<f32>,
        index: Option<excerpt::Index>,
    ) -> Self {
        let width = codestream.canvas.0 - codestream.origin.0;
        let height = codestream.canvas.1 - codestream.origin.1;
        let (tw, th) = codestream.tile;
        let overviews = (1..u32::from(codestream.resolutions))
            .take_while(|factor| tw % (1 << factor) == 0 && th % (1 << factor) == 0)
            .map(|factor| Overview {
                image: factor as usize,
                width: pyramid::level_size(width, factor),
                height: pyramid::level_size(height, factor),
                chunk: (tw >> factor, th >> factor),
            })
            .collect();
        let depth = u32::from(codestream.components) * SAMPLE_BYTES;
        Self {
            grid: Grid { tiled: true, chunk: codestream.tile, overviews, depth },
            tiles: codestream.grid(),
            len,
            format,
            precision: codestream.precision,
            nodata,
            stretch: RefCell::new(None),
            index,
        }
    }

    /// Читается ли тайл выдержкой по TLM, а не обходом SOT.
    pub fn indexed(&self) -> bool {
        self.index.is_some()
    }
}

pub fn describe(mut reader: Metered, len: u64) -> Result<Info, String> {
    // Длина берётся в `u64` и сравнивается там же: у модуля `usize`
    // тридцатидвухбитный, и переведи мы её первой, файл в четыре гигабайта с
    // сотней байт дал бы голову в сотню байт — а это не отказ чтения, а
    // спокойный ответ «нет сигнатуры» про совершенно годный растр.
    let mut head = vec![0u8; len.min(HEAD as u64) as usize];
    reader.read_exact(&mut head).map_err(|e| format!("jp2: чтение заголовка: {}", e))?;
    let (width, height) = header_dims(&head)?;
    let layout = codestream(&head)?;
    // Ненулевое начало холста сдвигает уменьшенную решётку декодера на пиксель
    // против лестницы пирамиды (ADR 0002): такой файл не читается, а не
    // читается со швами.
    if layout.origin != (0, 0) || layout.tile_origin != (0, 0) {
        return Err(format!(
            "jp2: начало холста {:?} и сетки тайлов {:?} не в нуле — уменьшенная решётка \
             декодера разошлась бы с лестницей пирамиды",
            layout.origin, layout.tile_origin
        ));
    }
    if (layout.canvas.0 - layout.origin.0, layout.canvas.1 - layout.origin.1) != (width, height) {
        return Err("jp2: размеры контейнера и кодстрима разошлись".to_string());
    }
    let (across, down) = layout.grid();
    // Индекс строится здесь же, из той же головы: тайл по сети читается
    // выдержкой, только если TLM есть и сходится с файлом, и об этом говорит
    // строка описания — по ней меряется гранула.
    let index = excerpt::Index::of(&head, len);
    veldsdk::log::debug!(target: "perf",
        "jp2 {}×{}: тайлов {}×{} по {}×{}, компонент {}, разрешений {}, \
         прогрессия {}, слоёв {}, тайл-партов в TLM {}, PLT у первого тайл-парта {}, GML {}, \
         тайл читается {}",
        width, height, across, down, layout.tile.0, layout.tile.1,
        layout.components, layout.resolutions, layout.progression, layout.layers,
        layout.tlm_parts.map_or("нет TLM".to_string(), |n| n.to_string()),
        layout.plt_first.map_or("не видно", |plt| if plt { "есть" } else { "нет" }),
        if gml_text(&head).is_some() { "есть" } else { "нет" },
        match &index {
            Ok(index) => format!("выдержкой по TLM ({} тайл-партов, префикс по разрешениям {})",
                index.parts(), if index.coding().prefixed() { "возможен" } else { "невозможен" }),
            Err(why) => format!("обходом SOT: {}", why),
        });

    let format = match head.starts_with(CODESTREAM_MAGIC) {
        true => Format::J2k,
        false => Format::Jp2,
    };
    let placement = gml_placement(&head, width, height);
    let nodata = placement.is_some().then_some(0.0);
    let mut info = Info::plain(width, height, Kind::Jp2(Layout::of(&layout, len, format, nodata, index.ok())));
    info.placement = placement;
    Ok(info)
}

/// Привязка из коробки GMLJP2 — единственное место, где JP2 говорит о Земле.
///
/// Так лежит гранула Sentinel-2: коробка `asoc` с меткой `gml.data`, внутри
/// вторая с `gml.root-instance`, а в ней GML с прямоугольной решёткой.
/// Второго способа — вырожденного GeoTIFF в коробке `uuid` — у Sentinel-2 нет
/// вовсе, и разбирать его незачем, пока не встретится файл, который его несёт.
///
/// `None` — коробки нет, форма не та или решётка не про этот растр. Молчание,
/// а не отказ: JP2 без привязки — обычное дело, и такой снимок ложится по
/// контуру каталога.
fn gml_placement(head: &[u8], width: u32, height: u32) -> Option<Placement> {
    let whole = gml_text(head)?;
    // Читается только сама решётка, а не весь документ. Рядом в GML лежит
    // `gml:boundedBy` — тот же `gml:pos`, но в градусах и в другой системе, — и
    // первое вхождение по всему тексту достало бы его: снимок уехал бы на сотню
    // километров, не сказав ни слова.
    let text = slice_of(&whole, "<gml:RectifiedGrid", "</gml:RectifiedGrid>")?;

    // Код системы берётся из начала решётки, а не откуда придётся: srsName
    // стои́т и у других элементов, и чужой сюда попадать не должен.
    let origin = slice_of(text, "<gml:origin", "</gml:origin>")?;
    let epsg = after(origin, "EPSG::")?.parse::<u32>().ok()?;
    // Ноль и user-defined кодом не являются, и наружу их пускать нельзя:
    // `Placement` тем и определён, что нуля в нём не бывает (см. types.proto).
    if epsg == 0 || epsg == 32767 || !easting_first(epsg) {
        return None;
    }
    let (ox, oy) = pair(numbers(after_tag(origin, "<gml:pos>")?)?)?;

    let mut offsets = text.match_indices("<gml:offsetVector").filter_map(|(at, _)| {
        pair(numbers(after_tag(&text[at..], ">")?)?)
    });
    let (first, second) = (offsets.next()?, offsets.next()?);
    let ((x_per_i, y_per_i), (x_per_j, y_per_j)) = ordered(first, second);

    // Решётка обязана быть про этот растр. Разойдись она с ним — она описывает
    // не его, и натянутая всё равно положила бы снимок мимо себя, причём молча.
    //
    // Границы решётки читаются только на эту сверку. По букве GML это смещения
    // от начала, и тогда при `low = 1 1` первый отсчёт лежал бы в
    // `origin + v1 + v2`; гранула Sentinel-2 пишет туда единичную нумерацию
    // отсчётов, и следование букве сдвинуло бы всякую её плитку на пиксель.
    // Поэтому начало вешается на пиксель (0, 0), а `low` в него не входит.
    let (low, high) = (
        pair(numbers(after_tag(text, "<gml:low>")?)?)?,
        pair(numbers(after_tag(text, "<gml:high>")?)?)?,
    );
    let across = high.0 - low.0 + 1.0;
    let down = high.1 - low.1 + 1.0;
    if across != f64::from(width) || down != f64::from(height) {
        return None;
    }
    // Не-число прошло бы все проверки выше: оно не равно ничему, в том числе
    // самому себе. Дальше по нему считается варп-сетка, и там его уже не
    // поймать (тот же инвариант держит `tiff::usable_step`).
    if ![ox, oy, x_per_i, y_per_i, x_per_j, y_per_j].iter().all(|v| v.is_finite()) {
        return None;
    }

    // Начало решётки — центр первого отсчёта, а наружу уезжает угол пикселя
    // (см. `Placement` в types.proto): та же конвенция, что снимает полпикселя
    // у `RasterPixelIsPoint` в GeoTIFF.
    Some(Placement {
        epsg,
        affine: [
            x_per_i,
            x_per_j,
            ox - (x_per_i + x_per_j) / 2.0,
            y_per_i,
            y_per_j,
            oy - (y_per_i + y_per_j) / 2.0,
        ],
    })
}

/// Текст GML из вложенных коробок `asoc`. Ищется по содержимому, а не по
/// порядку: меток внутри бывает несколько, и та, что нужна, — при решётке.
fn gml_text(head: &[u8]) -> Option<String> {
    fn dig(buf: &[u8], depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let mut at = 0usize;
        while at + 8 <= buf.len() {
            let length = u32::from_be_bytes(buf.get(at..at + 4)?.try_into().ok()?) as u64;
            let kind = buf.get(at + 4..at + 8)?;
            // Нулевая длина — «до конца», единица — длина следующей восьмёркой.
            let (length, body) = match length {
                0 => ((buf.len() - at) as u64, at + 8),
                1 => (u64::from_be_bytes(buf.get(at + 8..at + 16)?.try_into().ok()?), at + 16),
                _ => (length, at + 8),
            };
            let end = usize::try_from(at as u64 + length).ok()?.min(buf.len());
            if body > end {
                return None;
            }
            let inner = &buf[body..end];
            let found = match kind {
                b"asoc" => dig(inner, depth + 1),
                b"xml " => String::from_utf8(inner.to_vec())
                    .ok()
                    .filter(|text| text.contains("RectifiedGrid")),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
            at = end.max(at + 8);
        }
        None
    }
    dig(head, 0)
}

/// Хвост строки после первого вхождения — до конца строки или до `<`.
fn after<'a>(text: &'a str, mark: &str) -> Option<&'a str> {
    let tail = &text[text.find(mark)? + mark.len()..];
    Some(&tail[..tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len())])
}

/// Содержимое элемента, открывающий тег которого только что назвали.
fn after_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let tail = &text[text.find(tag)? + tag.len()..];
    Some(&tail[..tail.find('<').unwrap_or(tail.len())])
}

/// Кусок текста между открывающим и закрывающим — чтобы читать элемент, а не
/// весь документ.
fn slice_of<'a>(text: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let start = text.find(from)?;
    let end = text[start..].find(to)? + start;
    Some(&text[start..end])
}

/// Идут ли координаты системы «восток, север» — в том порядке, в каком их
/// пишет GML.
///
/// Порядок этот у системы свой: у зон UTM и веб-Меркатора восток первым, а у
/// зон Гаусса-Крюгера и местных систем — север. Перепутанный, он кладёт снимок
/// накрест, и заметить это по числам нельзя: они настоящие.
///
/// Здесь названы только те, чей порядок известен наверняка. Прочие остаются без
/// привязки из файла — и это верный размен: снимок ложится по контуру каталога,
/// то есть примерно, а не накрест.
fn easting_first(epsg: u32) -> bool {
    matches!(epsg, 3857 | 32601..=32660 | 32701..=32760)
}

/// Векторы сдвига в порядке «на столбец, на строку».
///
/// Порядок их задаёт файл своими `axisLabels`, и записывают его по-разному.
/// Переставленную пару видно по форме: у растра без поворота первый вектор идёт
/// вдоль строки, то есть его восточная составляющая не нулевая. Нулевая при
/// ненулевой северной — оси переставлены.
///
/// Разбирается только этот случай, однозначный. У повёрнутого растра обе
/// составляющие ненулевые, догадываться там не о чем, и пара берётся как есть.
fn ordered(first: (f64, f64), second: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    match first.0 == 0.0 && first.1 != 0.0 && second.0 != 0.0 && second.1 == 0.0 {
        true => (second, first),
        false => (first, second),
    }
}

/// Числа через пробел.
fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split_whitespace().map(|word| word.parse::<f64>().ok()).collect()
}

/// Ровно два числа: пара координат либо пара шагов. Больше или меньше — форма
/// не та, и додумывать её нечем.
fn pair(values: Vec<f64>) -> Option<(f64, f64)> {
    match values.as_slice() {
        [first, second] => Some((*first, *second)),
        _ => None,
    }
}

/// Чанки JP2 за трейтом драйвера: тайл на факторе образа, растяг файла и
/// ключ «нет данных». Кодек с индексом — на тайл: его поток и есть выдержка
/// этого тайла; без индекса — на фактор над всем файлом, потому что фактор
/// задаётся при открытии, а обход SOT кодек ведёт от последнего прочитанного.
pub struct Chunks<'a> {
    resource: u64,
    layout: &'a Layout,
    bytes: Rc<Cell<u64>>,
    walker: Option<(u32, Decoder<Metered>)>,
}

impl<'a> Chunks<'a> {
    pub fn new(resource: u64, layout: &'a Layout, bytes: Rc<Cell<u64>>) -> Self {
        Self { resource, layout, bytes, walker: None }
    }

    /// Тайл `index` на факторе `factor` — плоскости и заголовок кодека
    /// читателю `read`, пока они живы.
    fn decode<T>(
        &mut self,
        factor: u32,
        index: u32,
        read: impl FnOnce(&Tile<'_>, Header) -> Result<T, String>,
    ) -> Result<T, String> {
        match &self.layout.index {
            Some(excerpt) => {
                let mut decoder = self.excerpt(excerpt, index, factor)?;
                let header = decoder.header();
                decoder.tile(index, |tile| read(tile, header))
            }
            None => {
                let decoder = self.walker(factor)?;
                let header = decoder.header();
                decoder.tile(index, |tile| read(tile, header))
            }
        }
    }

    /// Кодек над выдержкой тайла: пробы его тайл-партов читаются здесь,
    /// сборка и чтение остального — у `excerpt`. Счётчик `bytes` здесь —
    /// сумма проб и окон, а у кодека над всем файлом (`Metered`) — дальняя
    /// достигнутая позиция; в прогресс оба идут как «прочитано», и сумма
    /// перевалит за размер файла только при повторном чтении одного места.
    fn excerpt(&self, index: &excerpt::Index, tile: u32, factor: u32) -> Result<Decoder<excerpt::Reader>, String> {
        let mut probes = Vec::new();
        for part in index.parts_of(tile) {
            let size = part.len.min(excerpt::PROBE);
            let probe = veldsdk::abi::resource_read(self.resource, part.offset, size)
                .map_err(|e| format!("jp2: проба тайл-парта тайла {}: {}", tile, e))?;
            self.bytes.set(self.bytes.get() + probe.len() as u64);
            probes.push(probe);
        }
        let segments = excerpt::assemble(index, tile, factor, probes).map_err(|why| format!("jp2: {}", why))?;
        // Куски файла в выдержке — носителю наперёд, прежде чем кодек пойдёт
        // по ним окнами: у подробного уровня это весь тайл-парт, и по промахам
        // он ехал бы блок за блоком.
        let pieces: Vec<(u64, u64)> = segments
            .iter()
            .filter_map(|segment| match segment {
                excerpt::Segment::File { offset, len } => Some((*offset, *len)),
                excerpt::Segment::Bytes(_) => None,
            })
            .collect();
        if let Err(why) = veldsdk::abi::resource_prefetch(self.resource, &pieces) {
            veldsdk::log::debug!(target: "decode", "jp2: куски выдержки тайла {} наперёд не привезены: {}", tile, why);
        }
        let reader = excerpt::Reader::over(self.resource, segments, self.bytes.clone());
        let len = reader.len();
        opened(Decoder::open(reader, len, self.layout.format, factor))
    }

    /// Кодек над всем файлом на факторе `factor` — готовый или открытый заново.
    fn walker(&mut self, factor: u32) -> Result<&mut Decoder<Metered>, String> {
        if !matches!(&self.walker, Some((at, _)) if *at == factor) {
            let reader = Metered::new(self.resource, self.layout.len, self.bytes.clone());
            let decoder = opened(Decoder::open(reader, self.layout.len, self.layout.format, factor))?;
            self.walker = Some((factor, decoder));
        }
        Ok(&mut self.walker.as_mut().expect("декодер только что открыт").1)
    }

    /// Растяг файла — готовый либо посчитанный по выборке самой мелкой копии
    /// и запомненный. Байтовым отсчётам растяг не нужен: у них тождество и
    /// ключ «нет данных», и кодек ради этого не открывается.
    fn mapping(&mut self) -> Result<Mapping, String> {
        if let Some(ready) = *self.layout.stretch.borrow() {
            return Ok(ready);
        }
        // Раскладка берётся ссылкой до декодера: тот заимствует `self` целиком.
        let layout = self.layout;
        let nodata = layout.nodata;
        let built = match layout.precision <= 8 {
            true => Mapping::identity(nodata),
            false => {
                // Выборка — четыре тайла вразброс по сетке кодстрима, на самом
                // мелком факторе, прореженные до общего порога выборки; цвет
                // берётся из первого компонента. Сетка тайлов у кодека одна на
                // все факторы, и считать её по уменьшенному чанку нельзя.
                let coarsest = layout.grid.overviews.len() as u32;
                let mut values = Vec::new();
                let total = layout.tiles.0 * layout.tiles.1;
                let mut picks = [0, total / 3, 2 * total / 3, total.saturating_sub(1)].to_vec();
                picks.dedup();
                for &index in &picks {
                    self.decode(coarsest, index, |tile, header| {
                        let offset = sample_offset(header);
                        let plane = tile.planes[0];
                        let step = (plane.len() * picks.len() / radiometry::STRETCH_SAMPLES).max(1);
                        for at in (0..plane.len()).step_by(step) {
                            let v = (plane[at] + offset) as f32;
                            if radiometry::is_data(v, nodata) {
                                values.push(v);
                            }
                        }
                        Ok(())
                    })?;
                }
                match percentile_stretch(&mut values) {
                    Some((lo, hi)) => Mapping::stretched(lo, hi, nodata),
                    None => Mapping::identity(nodata),
                }
            }
        };
        *layout.stretch.borrow_mut() = Some(built);
        Ok(built)
    }
}

/// Открытый кодек, чей заголовок раскладывается в RGBA: компоненты одной
/// решётки и разрядности, от одного до четырёх.
fn opened<R: Read + Seek>(decoder: Result<Decoder<R>, String>) -> Result<Decoder<R>, String> {
    let decoder = decoder.map_err(|why| format!("jp2: {}", why))?;
    let header = decoder.header();
    if header.uneven {
        return Err("jp2: компоненты разной решётки или разрядности не разложить в RGBA".to_string());
    }
    if header.components == 0 || header.components > 4 {
        return Err(format!("jp2: {} компонентов не разложить в RGBA", header.components));
    }
    Ok(decoder)
}

impl Chunked for Chunks<'_> {
    fn chunk(&mut self, image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String> {
        let mapping = self.mapping()?;
        self.decode(image as u32, index, |tile, header| Ok((rgba(tile, header, &mapping), tile.width, tile.height)))
    }

    /// С индексом известны пробы тайл-партов заказанных тайлов — они и едут
    /// наперёд; куски за пробами называются по тайлу, когда собрана выдержка
    /// (см. [`Chunks::excerpt`]). Без индекса кодек ведёт обход сам, и
    /// предсказать его чтения нечем.
    fn prefetch(&mut self, _image: usize, indices: &[u32]) -> Result<(), String> {
        let Some(index) = &self.layout.index else { return Ok(()) };
        let probes: Vec<(u64, u64)> = indices
            .iter()
            .flat_map(|&tile| index.parts_of(tile))
            .map(|part| (part.offset, part.len.min(excerpt::PROBE)))
            .collect();
        veldsdk::abi::resource_prefetch(self.resource, &probes).map_err(|e| e.to_string())
    }
}

/// Сдвиг, переводящий знаковый отсчёт в беззнаковый той же разрядности.
fn sample_offset(header: Header) -> i32 {
    match header.signed {
        true => 1 << header.precision.saturating_sub(1).min(30),
        false => 0,
    }
}

/// Плоскости тайла — в RGBA по раскладке пикселя: отсчёты до байта
/// дотягиваются до байта сдвигом, шире байта идут через растяг файла.
fn rgba(tile: &Tile<'_>, header: Header, mapping: &Mapping) -> Vec<u8> {
    let pixels = (tile.width as usize) * (tile.height as usize);
    let channels = tile.planes.len();
    let pixel = Pixel::named(channels);
    let offset = sample_offset(header);
    if header.precision <= 8 {
        let top = (1i32 << header.precision) - 1;
        let shift = 8 - header.precision;
        let mut samples = vec![0u8; pixels * channels];
        for (c, plane) in tile.planes.iter().enumerate() {
            for (at, value) in plane.iter().take(pixels).enumerate() {
                samples[at * channels + c] = (((value + offset).clamp(0, top)) << shift) as u8;
            }
        }
        return mapping.rgba(&Samples::U8(&samples), pixel, pixels);
    }
    let mut samples = vec![0u16; pixels * channels];
    for (c, plane) in tile.planes.iter().enumerate() {
        for (at, value) in plane.iter().take(pixels).enumerate() {
            samples[at * channels + c] = (value + offset).clamp(0, i32::from(u16::MAX)) as u16;
        }
    }
    mapping.rgba(&Samples::U16(&samples), pixel, pixels)
}

/// Точечное чтение тайлов уровня — драйвером по тайлам кодстрима.
pub fn produce_direct(
    resource: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::new(resource, layout, bytes.clone());
    grid::direct(&mut chunks, &layout.grid, (info.width, info.height), level, wants, emit)
}

/// Проход по тайлам кодстрима на родном разрешении — драйвером.
pub fn produce_pass(
    resource: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::new(resource, layout, bytes.clone());
    grid::pass(&mut chunks, &layout.grid, (info.width, info.height), emit)
}

// ── Разбор заголовка без декодера ──────────────────────────────

/// Сигнатура контейнера JP2 (signature box) и сырого кодстрима (SOC+SIZ).
pub const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
pub const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";

/// Куда шагнуть за боксом длины `len`, начавшимся в `at`.
///
/// Считается в `u64` и обрезается по длине головы, и обе половины
/// обязательны. Длину бокса пишет файл, а файл бывает битым: объявив четыре
/// гигабайта, он уводит позицию под самый конец адресного пространства — у
/// модуля оно тридцатидвухбитное. Дальше заворачивает уже `at + 8` в условии
/// цикла, заворачивает молча (проверок переполнения в релизе нет), и виток
/// входит в тело с позицией больше конца среза — то есть в срез задом наперёд,
/// то есть в трап всего инстанса.
///
/// Обрезка заодно кончает обход: шаг за край головы цикла не проходит.
fn step(at: usize, len: u64, limit: usize) -> usize {
    (at as u64).saturating_add(len).min(limit as u64) as usize
}

/// Размеры растра из головы файла: у контейнера — box `jp2h`→`ihdr`, у сырого
/// кодстрима — маркер SIZ. Ошибка — заголовок битый либо не поместился в
/// прочитанную голову.
pub fn header_dims(head: &[u8]) -> Result<(u32, u32), String> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return siz_dims(&head[2..]);
    }
    if !head.starts_with(JP2_MAGIC) {
        return Err("jp2: нет сигнатуры".to_string());
    }

    // Боксы верхнего уровня: [длина u32][тип 4Б][тело]. Длина 1 — расширенная
    // (следующие 8 байт), 0 — до конца файла.
    let mut at = 0usize;
    while at + 8 <= head.len() {
        let len = u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as u64;
        let kind = &head[at + 4..at + 8];
        let (body, next) = match len {
            0 => (at + 8, head.len()),
            1 => {
                if at + 16 > head.len() {
                    break;
                }
                let xlen = u64::from_be_bytes(head[at + 8..at + 16].try_into().unwrap());
                (at + 16, step(at, xlen, head.len()))
            }
            _ => (at + 8, step(at, len, head.len())),
        };
        if kind == b"jp2h" {
            // Внутри супербокса ihdr обязан идти первым (ISO 15444-1, I.5.3):
            // [длина][ihdr][высота u32][ширина u32][каналы u16]…
            let ihdr = body;
            if ihdr + 16 > head.len() || &head[ihdr + 4..ihdr + 8] != b"ihdr" {
                return Err("jp2: jp2h без ihdr первым боксом".to_string());
            }
            let height = u32::from_be_bytes(head[ihdr + 8..ihdr + 12].try_into().unwrap());
            let width = u32::from_be_bytes(head[ihdr + 12..ihdr + 16].try_into().unwrap());
            return Ok((width, height));
        }
        if next <= at {
            break;
        }
        at = next;
    }
    Err("jp2: заголовок не найден в голове файла".to_string())
}

/// SIZ: [маркер FF51][Lsiz u16][Rsiz u16][Xsiz u32][Ysiz u32][XOsiz][YOsiz]…
/// Размер растра — область холста за вычетом смещения.
fn siz_dims(from_siz: &[u8]) -> Result<(u32, u32), String> {
    if from_siz.len() < 22 || from_siz[0] != 0xFF || from_siz[1] != 0x51 {
        return Err("jp2: кодстрим без SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(from_siz[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, xo, yo) = (be32(6), be32(10), be32(14), be32(18));
    if xsiz <= xo || ysiz <= yo {
        return Err("jp2: пустой холст в SIZ".to_string());
    }
    Ok((xsiz - xo, ysiz - yo))
}

/// Раскладка кодстрима: чем он нарезан и на сколько разрешений разложен.
///
/// Показу не нужна — это мерка цены чтения куска, и сегодня её только
/// печатает журнал описания (см. [`describe`]).
/// Единица чтения у JPEG 2000 — **тайл-парт**, а не пакет: тело тайла читается
/// сплошняком. Значит «прочитать кусок картинки» стои́т ровно столько тайлов,
/// сколько этот кусок задевает, и у нарезанного на один тайл файла выбор куска
/// не экономит ни байта.
///
/// `resolutions` — сколько ступеней вейвлета записано. Грубее последней
/// декодер не отдаёт, и такие уровни пирамиды добираются своим делением
/// пополам.
///
/// Три поля про то, найдётся ли тайл без обхода всего файла: `tlm_parts` —
/// сколько тайл-партов перечислено в TLM главного заголовка (`None` — TLM
/// нет, и адрес тайла узнаётся только обходом SOT); `plt_first` — есть ли PLT
/// в заголовке первого тайл-парта (`None` — тот в голову не поместился);
/// `progression` — порядок прогрессии из COD. Записываются в журнал описания:
/// по ним решается, как читать гранулу по сети (см. `docs/decisions/0002`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codestream {
    /// Холст и его начало (`Xsiz`/`Ysiz`, `XOsiz`/`YOsiz`).
    pub canvas: (u32, u32),
    pub origin: (u32, u32),
    /// Шаг сетки тайлов и её начало (`XTsiz`/`YTsiz`, `XTOsiz`/`YTOsiz`).
    pub tile: (u32, u32),
    pub tile_origin: (u32, u32),
    pub components: u16,
    /// Разрядность первого компонента (`Ssiz`): до байта отсчёты идут без
    /// растяга.
    pub precision: u8,
    pub resolutions: u8,
    pub progression: u8,
    /// Слоёв качества из COD: при одном слое пакеты тайл-парта в LRCP идут
    /// по разрешениям, и его префикс — грубые уровни.
    pub layers: u16,
    pub tlm_parts: Option<u32>,
    pub plt_first: Option<bool>,
}

impl Codestream {
    /// Тайлов по горизонтали и вертикали (ISO 15444-1, B.3).
    pub fn grid(&self) -> (u32, u32) {
        let across = |canvas: u32, origin: u32, step: u32| match step {
            0 => 0,
            _ => (canvas.saturating_sub(origin)).div_ceil(step),
        };
        (
            across(self.canvas.0, self.tile_origin.0, self.tile.0),
            across(self.canvas.1, self.tile_origin.1, self.tile.1),
        )
    }
}

/// Где в файле длиной `len` лежит кодстрим: начало в голове и конец в файле.
/// У голого кодстрима это весь файл, у контейнера — тело коробки `jp2c`, и
/// длина её нулём значит «до конца файла». `None` — коробки в прочитанную
/// голову не поместилось.
pub(super) fn codestream_span(head: &[u8], len: u64) -> Option<(usize, u64)> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return Some((0, len));
    }
    let mut at = 0usize;
    while at + 8 <= head.len() {
        let declared = u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as u64;
        let kind = &head[at + 4..at + 8];
        let (body, declared) = match declared {
            0 => (at + 8, 0),
            1 => {
                if at + 16 > head.len() {
                    return None;
                }
                (at + 16, u64::from_be_bytes(head[at + 8..at + 16].try_into().unwrap()))
            }
            _ => (at + 8, declared),
        };
        if kind == b"jp2c" {
            let end = match declared {
                0 => len,
                _ => (at as u64).saturating_add(declared).min(len),
            };
            return Some((body, end));
        }
        let next = match declared {
            0 => head.len(),
            _ => step(at, declared, head.len()),
        };
        if next <= at {
            return None;
        }
        at = next;
    }
    None
}

/// Обход сегментов главного заголовка за SIZ: маркер и тело сегмента (без
/// маркера и длины), пока не встретится SOT. Один на раскладку
/// ([`codestream`]) и индекс тайл-партов (`excerpt::Index`): двум обходчикам
/// одного заголовка незачем расходиться в том, где он кончается.
///
/// Длина у сегмента стои́т сразу за маркером и считает саму себя, так что
/// следующий маркер — через `2 + L`. Сегмент, не поместившийся в голову,
/// кончает обход молча: голова конечна, и это не порча файла.
pub(super) struct Segments<'a> {
    cs: &'a [u8],
    walk: usize,
    sot: Option<usize>,
}

pub(super) fn segments(cs: &[u8]) -> Segments<'_> {
    // SOC — два байта без длины, дальше SIZ со своей длиной за маркером.
    let lsiz = cs.get(4..6).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
    Segments { cs, walk: 4 + lsiz, sot: None }
}

impl<'a> Segments<'a> {
    /// Следующий сегмент; `None` — дошли до SOT либо голова кончилась.
    /// Не-маркер там, где положено быть маркеру, — отказ: заголовок битый.
    pub(super) fn next(&mut self) -> Result<Option<(u8, &'a [u8])>, String> {
        let cs = self.cs;
        if self.sot.is_some() || self.walk + 4 > cs.len() {
            return Ok(None);
        }
        if cs[self.walk] != 0xFF {
            return Err("jp2: главный заголовок кончился не маркером".to_string());
        }
        let marker = cs[self.walk + 1];
        if marker == 0x90 {
            self.sot = Some(self.walk);
            return Ok(None);
        }
        let length = usize::from(u16::from_be_bytes([cs[self.walk + 2], cs[self.walk + 3]]));
        if length < 2 || self.walk + 2 + length > cs.len() {
            self.walk = cs.len();
            return Ok(None);
        }
        let body = &cs[self.walk + 4..self.walk + 2 + length];
        self.walk += 2 + length;
        Ok(Some((marker, body)))
    }

    /// Где стоит первый SOT, если обход до него дошёл.
    pub(super) fn sot(&self) -> Option<usize> {
        self.sot
    }
}

/// Раскладка из головы: маркер SIZ даёт холст и сетку тайлов, первый COD —
/// число разрешений.
///
/// COD ищется обходом маркеров, а не по смещению: между SIZ и COD стоя́т
/// необязательные сегменты, и порядок их файл выбирает сам. Обход кончается на
/// SOT — дальше идут тайлы, а нам нужен главный заголовок.
pub fn codestream(head: &[u8]) -> Result<Codestream, String> {
    let (at, _) = codestream_span(head, head.len() as u64).ok_or("jp2: кодстрим не найден в голове файла")?;
    let cs = &head[at..];
    if cs.len() < 43 || cs[0] != 0xFF || cs[1] != 0x4F || cs[2] != 0xFF || cs[3] != 0x51 {
        return Err("jp2: кодстрим без SOC и SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let be16 = |at: usize| u16::from_be_bytes(cs[at..at + 2].try_into().unwrap());
    // SOC — два байта без длины, дальше сегмент SIZ: [маркер 2][Lsiz 2][Rsiz 2]
    // [Xsiz 4][Ysiz 4][XOsiz 4][YOsiz 4][XTsiz 4][YTsiz 4][XTOsiz 4][YTOsiz 4]
    // [Csiz 2][Ssiz 1 XRsiz 1 YRsiz 1]… Смещения считаются от маркера SIZ, а
    // не от начала кодстрима: SOC перед ним свою пару байт занимает. В Ssiz
    // младшие семь бит — разрядность без единицы.
    let siz = 2usize;
    let mut layout = Codestream {
        canvas: (be32(siz + 6), be32(siz + 10)),
        origin: (be32(siz + 14), be32(siz + 18)),
        tile: (be32(siz + 22), be32(siz + 26)),
        tile_origin: (be32(siz + 30), be32(siz + 34)),
        components: be16(siz + 38),
        precision: (cs[siz + 40] & 0x7F).saturating_add(1),
        resolutions: 0,
        progression: 0,
        layers: 0,
        tlm_parts: None,
        plt_first: None,
    };

    let mut walk = segments(cs);
    while let Some((marker, body)) = walk.next()? {
        // COD: [Scod 1][SGcod: порядок 1, слоёв 2, MCT 1][SPcod: ступеней 1…].
        // Ступеней вейвлета на единицу меньше, чем разрешений.
        if marker == 0x52 && body.len() >= 10 {
            layout.progression = body[1];
            layout.layers = u16::from_be_bytes([body[2], body[3]]);
            layout.resolutions = body[5].saturating_add(1);
        }
        // TLM: [Ztlm 1][Stlm 1][записи…]; в Stlm биты 4–5 — ширина Ttlm
        // (0, 1 или 2 байта), бит 6 — ширина Ptlm (2 или 4). Сегментов бывает
        // несколько, записи считаются по всем.
        if marker == 0x55 && body.len() >= 2 {
            let stlm = body[1];
            let entry = usize::from((stlm >> 4) & 3) + if stlm & 0x40 != 0 { 4 } else { 2 };
            let listed = ((body.len() - 2) / entry) as u32;
            layout.tlm_parts = Some(layout.tlm_parts.unwrap_or(0) + listed);
        }
    }
    if let Some(sot) = walk.sot() {
        layout.plt_first = first_tile_part_has_plt(cs, sot);
    }
    if layout.resolutions == 0 {
        return Err("jp2: кодстрим без COD в голове файла".to_string());
    }
    Ok(layout)
}

/// Есть ли PLT в заголовке первого тайл-парта, начинающегося маркером SOT в
/// `at`. `None` — заголовок в прочитанную голову не поместился: до SOD не
/// дошли и PLT не встретили.
fn first_tile_part_has_plt(cs: &[u8], at: usize) -> Option<bool> {
    // SOT: [Lsot 2 = 10][Isot 2][Psot 4][TPsot 1][TNsot 1]; дальше сегменты
    // заголовка тайл-парта до SOD.
    let mut walk = at + 12;
    while walk + 2 <= cs.len() && cs[walk] == 0xFF {
        match cs[walk + 1] {
            0x93 => return Some(false),
            0x58 => return Some(true),
            _ => {}
        }
        // У прочих сегментов есть длина; SOD и PLT решены выше, до неё.
        if walk + 4 > cs.len() {
            break;
        }
        let length = usize::from(u16::from_be_bytes([cs[walk + 2], cs[walk + 3]]));
        if length < 2 {
            break;
        }
        walk += 2 + length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Коробка JP2: длина, тип, содержимое. `extended` — длина восьмёркой
    /// после типа, как её пишет настоящая гранула Sentinel-2.
    fn box_of(kind: &[u8; 4], body: &[u8], extended: bool) -> Vec<u8> {
        let mut out = Vec::new();
        match extended {
            false => out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes()),
            true => out.extend_from_slice(&1u32.to_be_bytes()),
        }
        out.extend_from_slice(kind);
        if extended {
            out.extend_from_slice(&((body.len() + 16) as u64).to_be_bytes());
        }
        out.extend_from_slice(body);
        out
    }

    /// GMLJP2 так, как он лежит в грануле: две вложенные `asoc` с метками,
    /// внешняя — с длиной восьмёркой.
    fn gmljp2(gml: &str) -> Vec<u8> {
        let inner = [
            box_of(b"lbl ", b"gml.root-instance", false),
            box_of(b"xml ", gml.as_bytes(), false),
        ]
        .concat();
        let outer = [box_of(b"lbl ", b"gml.data", false), box_of(b"asoc", &inner, false)].concat();
        box_of(b"asoc", &outer, true)
    }

    /// Настоящий GML из гранулы `S2B_MSIL2A_20260901T104619_..._T42XWQ`,
    /// урезанный до того, что читается. Числа не выдуманы.
    const REAL_GML: &str = r#"<gml:FeatureCollection>
      <gml:RectifiedGridCoverage><gml:domainSet><gml:RectifiedGrid dimension="2">
        <gml:limits><gml:GridEnvelope>
          <gml:low>1 1</gml:low><gml:high>10980 10980</gml:high>
        </gml:GridEnvelope></gml:limits>
        <gml:origin><gml:Point gml:id="P0001" srsName="urn:ogc:def:crs:EPSG::32642">
          <gml:pos>499985 8999995</gml:pos></gml:Point></gml:origin>
        <gml:offsetVector srsName="urn:ogc:def:crs:EPSG::32642">10 0</gml:offsetVector>
        <gml:offsetVector srsName="urn:ogc:def:crs:EPSG::32642">0 -10</gml:offsetVector>
      </gml:RectifiedGrid></gml:domainSet></gml:RectifiedGridCoverage></gml:FeatureCollection>"#;

    /// Привязка Sentinel-2 читается из файла, и угол выходит круглым.
    ///
    /// Круглый угол — не украшение, а проверка полпикселя: начало решётки в GML
    /// названо центром первого отсчёта, и снятые пять метров дают ровно ту
    /// сотку, по которой нарезана плитка MGRS. Не снятые — сдвинули бы снимок
    /// на полпикселя, и заметить это было бы нечем.
    #[test]
    fn привязка_гранулы_читается_и_угол_выходит_круглым() {
        let head = gmljp2(REAL_GML);
        let said = gml_placement(&head, 10980, 10980).expect("гранула несёт привязку");

        assert_eq!(said.epsg, 32642, "UTM 42 северная");
        assert_eq!(said.affine, [10.0, 0.0, 499_980.0, 0.0, -10.0, 9_000_000.0]);
    }

    /// Читается решётка, а не весь документ.
    ///
    /// В GML рядом с ней лежит габарит, а у файлов побогаче — и чужие сетки с
    /// теми же именами тегов. Ищи мы по всему тексту, первым нашлось бы
    /// постороннее: система, границы и шаг уехали бы от чужого элемента, а
    /// числа при этом остались бы правдоподобными.
    ///
    /// Приманки здесь собраны нарочно — по одной на каждое читаемое поле.
    #[test]
    fn читается_решётка_а_не_весь_документ() {
        let with_decoys = format!(
            r#"<gml:FeatureCollection>
                 <gml:boundedBy><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326">
                   <gml:pos>81.0 66.0</gml:pos></gml:Envelope></gml:boundedBy>
                 <gml:GridEnvelope><gml:low>0 0</gml:low><gml:high>1 1</gml:high></gml:GridEnvelope>
                 <gml:offsetVector srsName="urn:ogc:def:crs:EPSG::4326">1 0</gml:offsetVector>
                 <gml:offsetVector srsName="urn:ogc:def:crs:EPSG::4326">0 -1</gml:offsetVector>
                 {}
               </gml:FeatureCollection>"#,
            REAL_GML
        );
        let said = gml_placement(&gmljp2(&with_decoys), 10980, 10980).expect("решётка на месте");
        assert_eq!(said.epsg, 32642, "система взята у решётки");
        assert_eq!(said.affine, [10.0, 0.0, 499_980.0, 0.0, -10.0, 9_000_000.0], "и шаг с началом");
    }

    /// Система, чей порядок осей неизвестен, привязки не даёт: у зон
    /// Гаусса-Крюгера север пишется первым, и взятая как есть пара положила бы
    /// снимок накрест — числа при этом настоящие, и поймать это нечем.
    #[test]
    fn система_с_неизвестным_порядком_осей_не_берётся() {
        let gk = REAL_GML.replace("EPSG::32642", "EPSG::28408");
        assert!(gml_placement(&gmljp2(&gk), 10980, 10980).is_none());

        let web = REAL_GML.replace("EPSG::32642", "EPSG::3857");
        assert!(gml_placement(&gmljp2(&web), 10980, 10980).is_some(), "веб-Меркатор известен");
    }

    /// Переставленные векторы сдвига возвращаются на места. У квадратной
    /// гранулы сверка размера решётки такую перестановку не ловит вовсе, а
    /// файлы с обоими порядками записи существуют.
    #[test]
    fn переставленные_векторы_возвращаются_на_места() {
        let swapped = REAL_GML
            .replace(">10 0</gml:offsetVector>", ">\u{1}</gml:offsetVector>")
            .replace(">0 -10</gml:offsetVector>", ">10 0</gml:offsetVector>")
            .replace(">\u{1}</gml:offsetVector>", ">0 -10</gml:offsetVector>");
        let said = gml_placement(&gmljp2(&swapped), 10980, 10980).expect("привязка читается");
        assert_eq!(said.affine, [10.0, 0.0, 499_980.0, 0.0, -10.0, 9_000_000.0]);
    }

    /// Не-число проходит всякое сравнение — оно не равно ничему, включая себя.
    /// Дальше по нему считается варп-сетка, и там его уже не поймать.
    #[test]
    fn не_число_в_привязке_не_берётся() {
        let broken = REAL_GML.replace("499985 8999995", "NaN 8999995");
        assert!(gml_placement(&gmljp2(&broken), 10980, 10980).is_none());
    }

    /// Решётка не про этот растр — привязки нет. Натянутая, она положила бы
    /// снимок мимо себя, и молча: числа-то настоящие, просто чужие.
    #[test]
    fn чужая_решётка_не_берётся() {
        let head = gmljp2(REAL_GML);
        assert!(gml_placement(&head, 10980, 10980).is_some(), "своя берётся");
        assert!(gml_placement(&head, 5490, 10980).is_none(), "ширина не та");
        assert!(gml_placement(&head, 10980, 5490).is_none(), "высота не та");
    }

    /// Ни коробки, ни формы — молчание, а не отказ: JP2 без привязки обычен, и
    /// такой снимок ложится по контуру каталога.
    #[test]
    fn без_коробки_привязки_нет_и_это_не_беда() {
        assert!(gml_placement(&[], 10980, 10980).is_none(), "пусто");
        assert!(gml_placement(b"not a jp2 at all", 10980, 10980).is_none(), "мусор");

        let empty = gmljp2("<gml:FeatureCollection>RectifiedGrid без чисел</gml:FeatureCollection>");
        assert!(gml_placement(&empty, 10980, 10980).is_none(), "форма не та");
    }


    /// Бокс с телом — как их пишет контейнер.
    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn jp2_head(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr_body = height.to_be_bytes().to_vec();
        ihdr_body.extend_from_slice(&width.to_be_bytes());
        ihdr_body.extend_from_slice(&[0, 3, 7, 7, 0, 0]); // каналы, бит, прочее

        let mut head = JP2_MAGIC.to_vec();
        head.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]); // хвост сигнатурного бокса
        head.extend_from_slice(&boxed(b"ftyp", b"jp2 \x00\x00\x00\x00jp2 "));
        head.extend_from_slice(&boxed(b"jp2h", &boxed(b"ihdr", &ihdr_body)));
        head
    }

    #[test]
    fn container_header_yields_dims() {
        assert_eq!(header_dims(&jp2_head(10980, 5490)), Ok((10980, 5490)));
    }

    /// Кодстрим: SOC, SIZ с холстом и сеткой тайлов, необязательный сегмент
    /// между ними и COD, потом SOT. Именно так и лежит настоящий файл, и
    /// именно поэтому COD ищется обходом, а не по смещению.
    fn codestream_bytes(canvas: (u32, u32), origin: (u32, u32), tile: (u32, u32), levels: u8)
    -> Vec<u8> {
        let mut out = CODESTREAM_MAGIC.to_vec(); // SOC + маркер SIZ
        let mut siz = 0u16.to_be_bytes().to_vec(); // Rsiz
        for value in [canvas.0, canvas.1, origin.0, origin.1, tile.0, tile.1, origin.0, origin.1] {
            siz.extend_from_slice(&value.to_be_bytes());
        }
        siz.extend_from_slice(&3u16.to_be_bytes()); // Csiz
        siz.extend_from_slice(&[7, 1, 1, 7, 1, 1, 7, 1, 1]); // Ssiz/XRsiz/YRsiz ×3
        out.extend_from_slice(&((siz.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&siz);

        // Необязательный сегмент перед COD — обход обязан через него перешагнуть.
        out.extend_from_slice(&[0xFF, 0x55, 0x00, 0x04, 0x00, 0x00]); // TLM

        let cod = [0u8, 0, 0, 1, 0, levels, 5, 5, 0, 0, 0];
        out.extend_from_slice(&[0xFF, 0x52]);
        out.extend_from_slice(&((cod.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&cod);
        out.extend_from_slice(&[0xFF, 0x90]); // SOT — дальше главного заголовка нет
        out
    }

    /// Что решает чтение по сети, читается из той же головы: записи TLM
    /// считаются по всем сегментам и по ширине полей из Stlm, PLT ищется в
    /// заголовке первого тайл-парта, прогрессия берётся из COD.
    #[test]
    fn tile_part_index_and_plt_are_read_from_the_head() {
        let mut raw = codestream_bytes((2048, 2048), (0, 0), (1024, 1024), 4);
        let sot_at = raw.len() - 2;
        raw.truncate(sot_at);
        // Второй TLM: Ztlm 1, Stlm 0x40 (Ttlm нет, Ptlm по 4 байта), три записи.
        raw.extend_from_slice(&[0xFF, 0x55, 0x00, 0x10, 0x01, 0x40]);
        raw.extend_from_slice(&[0; 12]);
        // SOT первого тайл-парта, его заголовок с PLT, потом SOD.
        raw.extend_from_slice(&[0xFF, 0x90, 0x00, 0x0A, 0, 0, 0, 0, 0, 0, 0, 1]);
        raw.extend_from_slice(&[0xFF, 0x58, 0x00, 0x03, 0x00]);
        raw.extend_from_slice(&[0xFF, 0x93]);
        let layout = codestream(&raw).expect("раскладка читается");
        assert_eq!(layout.tlm_parts, Some(3), "первый TLM пуст, второй перечисляет три тайл-парта");
        assert_eq!(layout.plt_first, Some(true));
        assert_eq!((layout.progression, layout.layers), (0, 1));

        // Без PLT до SOD — «нет»; голова, кончившаяся раньше SOD, — «не видно».
        let without = codestream_bytes((2048, 2048), (0, 0), (1024, 1024), 4);
        let mut bare = without.clone();
        bare.extend_from_slice(&[0x00, 0x0A, 0, 0, 0, 0, 0, 0, 0, 1, 0xFF, 0x93]);
        assert_eq!(codestream(&bare).unwrap().plt_first, Some(false));
        assert_eq!(codestream(&without).unwrap().plt_first, None);
        assert_eq!(codestream(&without).unwrap().tlm_parts, Some(0), "пустой TLM — сегмент есть, записей нет");
    }

    /// Раскладка читается из кодстрима: холст, сетка тайлов, компоненты,
    /// разрешения. Число разрешений на единицу больше записанных ступеней —
    /// файл пишет ступени, а нам нужны уровни, которые декодер отдаст.
    #[test]
    fn codestream_layout_is_read_from_the_main_header() {
        let raw = codestream_bytes((10980, 10980), (0, 0), (1024, 1024), 5);
        let layout = codestream(&raw).expect("раскладка читается");

        assert_eq!(layout.canvas, (10980, 10980));
        assert_eq!(layout.origin, (0, 0));
        assert_eq!(layout.tile, (1024, 1024));
        assert_eq!(layout.components, 3);
        assert_eq!(layout.resolutions, 6, "ступеней 5 — значит разрешений 6");
        assert_eq!(layout.grid(), (11, 11));
    }

    /// Тот же кодстрим внутри коробки `jp2c`: у контейнера главный заголовок
    /// лежит не с начала файла, и найти его — половина работы.
    #[test]
    fn codestream_is_found_inside_the_container() {
        let mut head = jp2_head(700, 40);
        head.extend_from_slice(&boxed(b"jp2c", &codestream_bytes((700, 40), (0, 0), (700, 40), 4)));
        let layout = codestream(&head).expect("раскладка читается и в контейнере");

        assert_eq!(layout.canvas, (700, 40));
        assert_eq!(layout.resolutions, 5);
        assert_eq!(layout.grid(), (1, 1), "один тайл на весь холст");
    }

    /// Нарезка считается от начала сетки, а не от нуля: у файла со смещением
    /// холста они разные, и деление нацело здесь солгало бы.
    #[test]
    fn the_tile_grid_counts_from_its_own_origin() {
        let raw = codestream_bytes((1000, 600), (40, 40), (256, 256), 3);
        let layout = codestream(&raw).expect("раскладка читается");

        assert_eq!(layout.origin, (40, 40));
        assert_eq!(layout.grid(), (4, 3), "(1000-40)/256 → 4, (600-40)/256 → 3");
    }

    /// Кодстрим без COD — не раскладка «по умолчанию», а отказ: число
    /// разрешений решает, до какого уровня действует фактор декодера, и
    /// придумать его нельзя.
    #[test]
    fn a_codestream_without_cod_is_refused() {
        let mut raw = codestream_bytes((700, 40), (0, 0), (700, 40), 4);
        let cod = raw.windows(2).position(|pair| pair == [0xFF, 0x52]).expect("COD на месте");
        raw[cod + 1] = 0x58; // RGN — сегмент есть, но не тот
        assert!(codestream(&raw).is_err());
    }

    #[test]
    fn raw_codestream_yields_dims() {
        // SOC + SIZ с холстом 700×40 без смещения.
        let mut head = CODESTREAM_MAGIC.to_vec();
        head.extend_from_slice(&41u16.to_be_bytes()); // Lsiz
        head.extend_from_slice(&0u16.to_be_bytes()); // Rsiz
        for value in [700u32, 40, 0, 0] {
            head.extend_from_slice(&value.to_be_bytes());
        }
        assert_eq!(header_dims(&head), Ok((700, 40)));
    }

    #[test]
    fn garbage_is_refused() {
        assert!(header_dims(b"PNG not really").is_err());
        assert!(header_dims(JP2_MAGIC).is_err()); // сигнатура есть, заголовка нет
    }

    /// Шаг за боксом не выходит за голову, какую бы длину бокс ни объявил.
    ///
    /// Проверяется правило, а не отсутствие паники, и это не придирка к форме:
    /// паника здесь бывает только там, где `usize` тридцатидвухбитный, то есть
    /// в самом модуле, а тест бежит нативно на шестидесяти четырёх — сложение
    /// «начало плюс четыре гигабайта» в нём не переполняется по построению, и
    /// порченый разбор прошёл бы мимо. Правило же одинаково на любой
    /// разрядности, и держит оно ровно то, ради чего написано.
    #[test]
    fn a_box_never_steps_past_the_head() {
        // Длина обычного бокса: на тридцати двух битах ей уже не остаётся
        // места под «плюс начало».
        assert_eq!(step(0, u64::from(u32::MAX), 4096), 4096);
        assert_eq!(step(4088, u64::from(u32::MAX), 4096), 4096);
        // Расширенная длина приезжает из файла целым `u64`, так что переполнить
        // можно и его — от `at` в этой сумме места не остаётся вовсе.
        assert_eq!(step(4088, u64::MAX, 4096), 4096);
        assert_eq!(step(4088, u64::MAX - 1, 4096), 4096);
        // Обычный бокс шагает ровно на свою длину.
        assert_eq!(step(16, 32, 4096), 48);
    }

    /// Ключ «нет данных»: ноль всеми цветовыми каналами — прозрачность, и
    /// только у файла, которому он назначен; знаковые отсчёты сдвигаются в
    /// беззнаковые, разрядность до байта дотягивается до байта.
    #[test]
    fn samples_become_rgba_by_the_file_rules() {
        let planes = [vec![0, 200, 0], vec![0, 100, 0], vec![0, 50, 7]];
        let tile = Tile { x0: 0, y0: 0, width: 3, height: 1, planes: planes.iter().map(|p| p.as_slice()).collect() };
        let header = Header { x0: 0, y0: 0, width: 3, height: 1, components: 3, precision: 8, signed: false, uneven: false };
        let keyed = rgba(&tile, header, &Mapping::identity(Some(0.0)));
        assert_eq!(&keyed[0..4], &[0, 0, 0, 0], "ноль всеми каналами — поле");
        assert_eq!(&keyed[4..8], &[200, 100, 50, 255]);
        assert_eq!(&keyed[8..12], &[0, 0, 7, 255], "ноль не всеми каналами — цвет");
        let plain = rgba(&tile, header, &Mapping::identity(None));
        assert_eq!(&plain[0..4], &[0, 0, 0, 255], "без ключа поле — чёрный цвет");

        let signed = [vec![-8, 7]];
        let tile = Tile { x0: 0, y0: 0, width: 2, height: 1, planes: signed.iter().map(|p| p.as_slice()).collect() };
        let header = Header { precision: 4, signed: true, components: 1, width: 2, ..header };
        let grey = rgba(&tile, header, &Mapping::identity(None));
        assert_eq!(&grey[0..4], &[0, 0, 0, 255], "минимум знакового — ноль");
        assert_eq!(&grey[4..8], &[240, 240, 240, 255], "максимум четырёх бит — почти байт");
    }

    /// Раскладка драйвера из главного заголовка: чанк — тайл кодстрима, копии —
    /// пока тайл делится на два, глубина — по плоскостям декодера.
    #[test]
    fn the_layout_follows_the_codestream() {
        let cs = Codestream {
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
        let layout = Layout::of(&cs, 130 << 20, Format::Jp2, Some(0.0), None);
        assert_eq!(layout.grid.chunk, (1024, 1024));
        assert_eq!(layout.tiles, (11, 11));
        assert_eq!(layout.grid.overviews.len(), 4, "четыре копии при пяти разрешениях");
        assert_eq!((layout.grid.overviews[3].width, layout.grid.overviews[3].chunk), (687, (64, 64)));
        assert_eq!(layout.grid.depth, 3 * SAMPLE_BYTES);

        // Копии кончаются там, где тайл перестаёт делиться на два: у 300
        // это фактор 3, у нечётного тайла копий нет вовсе.
        let even = Codestream { tile: (300, 300), canvas: (600, 600), ..cs };
        assert_eq!(Layout::of(&even, 1, Format::J2k, None, None).grid.overviews.len(), 2);
        let odd = Codestream { tile: (301, 301), canvas: (602, 602), ..cs };
        assert!(Layout::of(&odd, 1, Format::J2k, None, None).grid.overviews.is_empty());
    }
}
