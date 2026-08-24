//! NetCDF-4 (он же HDF5): измеренная величина → растр.
//!
//! Файл этот не картинка, а набор именованных величин: температура
//! поверхности, содержание газа в столбе воздуха, качество измерения, ошибка
//! измерения. Показать его — значит выбрать из них ту, которая и есть
//! измерение, узнать, где каждый её отсчёт лежит на Земле, и растянуть
//! значения в яркость. Правила выбора — CF, то есть те же, которыми файл
//! описывает себя сам (см. [`preferred`]); шаблонов имён миссий здесь нет.
//!
//! Читается он целиком, и это не лень. HDF5 раскладывает величину чанками по
//! всему файлу, а каталог чанков лежит там же вперемешку с ними: прочитать
//! «только нужное окно» нельзя, не прочитав сначала половину файла окольными
//! запросами. Отсюда потолки — [`FILE_CAP`] и [`PLANE_BUDGET`]: отказ до
//! чтения честнее, чем смерть посреди него.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use hdf5_pure::{AttrValue, DType, File};

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid::TILE;
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples, STRETCH_SAMPLES};
use super::{Info, Kind, Tie};

/// Сигнатура HDF5 — с неё начинается всякий NetCDF-4. Классический NetCDF-3
/// (`CDF\x01`) сюда не относится: это другой формат, и его здесь нет.
pub const MAGIC: &[u8] = b"\x89HDF\r\n\x1a\x0a";

/// Потолок самого файла. Читается он целиком (см. заголовок), а чтение идёт по
/// сети: у CDSE это порядка полутора мегабайт в секунду, то есть потолок здесь
/// — это «сколько ждать, пока на шаре не появится ничего». Минуты довольно;
/// спектральный куб Sentinel-5P на семьсот мегабайт стои́т получаса ожидания
/// ради ответа «это измерения, а не изображение».
const FILE_CAP: u64 = 96 * 1024 * 1024;

/// Потолок величины в памяти: отсчёты как они лежат в файле плюс они же
/// развёрнутые в f32. Обе копии живут одновременно — развёртка читает первую и
/// пишет вторую, — и лимит линейной памяти инстанса в 1 ГБ требует назвать
/// границу до чтения.
const PLANE_BUDGET: u64 = 512 * 1024 * 1024;

/// Потолок координатных решёток полосы съёмки: у неё широта и долгота лежат
/// поотсчётно, то есть двумя такими же полями, как сама величина. Мельче
/// потолка величины нарочно — привязка это добавка к показу, и платить за неё
/// столько же, сколько за сам снимок, незачем: без неё снимок ляжет по контуру
/// каталога.
const TIES_BUDGET: u64 = 64 * 1024 * 1024;

/// Сторона решётки опорных точек. Столько же, сколько несёт гранула
/// Sentinel-1: между узлами остаются десятки километров, и линейная
/// интерполяция внутри ячейки расходится с поверхностью на метры.
const TIE_GRID: u32 = 21;

/// Сколько всего может лежать в файле — предел обхода, а не ожидание. Обходятся
/// заголовки всех величин, и у файла с тысячами их обход стоил бы дороже
/// показа.
const MAX_DATASETS: usize = 512;
const MAX_DEPTH: usize = 8;

/// Сколько всего может быть прочитано пробами, прежде чем сдаться.
///
/// Счётом проб это не выражается: у гранулы SLSTR плоскость в 157×126
/// отсчётов, и пустых подряд перед заполненной бывает десяток («только над
/// океаном», «только над сушей», по алфавиту), — а у полосы Sentinel-5P одна
/// плоскость стои́т мегабайтов, и второй такой пробы уже жалко. Потолок тот
/// же, что у одной величины: сколько мы согласны держать в памяти ради показа,
/// столько же согласны и прочитать ради выбора.
const PROBE_BUDGET: u64 = PLANE_BUDGET;

/// То, что решено показывать, вместе с уже прочитанными отсчётами.
///
/// Сам файл здесь не держится: из него взято всё, что нужно показу. А вот
/// отсчёты держатся — годность величины видна только по ним, значит к концу
/// описания они уже прочитаны, и второе чтение стои́ло бы второго разбора
/// файла целиком.
pub struct Source {
    /// Отсчёты показываемой величины, развёрнутые в f32.
    ///
    /// Держатся с описания, а не читаются заново: годность величины видна
    /// только по ним (см. [`describe`]), значит к концу описания они уже
    /// прочитаны — а второе чтение стои́т второго разбора всего файла. Сам
    /// файл после этого не нужен и не держится: из него взято всё.
    values: Vec<f32>,
    /// Путь показываемой величины внутри файла.
    path: String,
    /// Чем в ней помечено «нет данных» (`_FillValue`).
    fill: Option<f32>,
    /// Как величина называется в файле по-человечески — для журнала.
    said: String,
}

impl Source {
    /// Источник без единого отсчёта — тестам, которым нужен вид разбора, а не
    /// его содержимое. Отдельной дверцей затем, что нутро источника закрыто
    /// намеренно: отсчёты сюда кладёт только разбор файла.
    #[cfg(test)]
    pub fn hollow() -> Self {
        Self { values: Vec::new(), path: String::new(), fill: None, said: String::new() }
    }
}

pub fn describe<R: Read + Seek>(mut reader: R, len: u64) -> Result<Info, String> {
    if len > FILE_CAP {
        return Err(format!(
            "NetCDF {} МБ: файл читается только целиком, а потолок — {} МБ",
            len / (1024 * 1024),
            FILE_CAP / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::with_capacity(len as usize);
    reader.read_to_end(&mut bytes).map_err(|e| format!("NetCDF: чтение файла: {}", e))?;
    let file = File::from_bytes(bytes).map_err(|e| format!("NetCDF: {}", e))?;

    let surveyed = survey(&file)?;
    let order = preferred(&surveyed);
    if order.is_empty() {
        return Err(explain(&surveyed));
    }

    // Пустая величина — не ответ, и узнаётся это только чтением. Гранула
    // Sentinel-3 несёт величины, снятые не над всякой поверхностью («только
    // над океаном»), и над сушей такая лежит сплошным `_FillValue`:
    // показанная, она даёт прозрачный кадр без единого слова о том, почему на
    // шаре ничего нет. Однотонная — ответ последней очереди: показать её можно
    // (ровное поле встаёт в середину шкалы), но всякая соседка с перепадом
    // говорит больше. Прочитанное при этом остаётся показу.
    let mut skipped: Vec<String> = Vec::new();
    let mut flat: Option<(&Item, Vec<f32>)> = None;
    let mut probed: u64 = 0;
    for chosen in &order {
        let Some((height, width)) = chosen.plane else { continue };
        probed += u64::from(width) * u64::from(height) * 4;
        if probed > PROBE_BUDGET {
            break;
        }
        let values = match plane(&file, &chosen.path, width, height) {
            Ok(values) => values,
            Err(why) => {
                skipped.push(why);
                continue;
            }
        };
        match spread(&values, chosen.fill) {
            Spread::Varying => {}
            Spread::Empty => {
                skipped.push(format!("'{}' пуста", chosen.path));
                continue;
            }
            Spread::Flat => {
                skipped.push(format!("'{}' однотонна", chosen.path));
                flat.get_or_insert((chosen, values));
                continue;
            }
        }
        return told(&file, &surveyed, chosen, values, &order, &skipped);
    }
    match flat {
        Some((chosen, values)) => told(&file, &surveyed, chosen, values, &order, &skipped),
        None => Err(match skipped.is_empty() {
            true => explain(&surveyed),
            false => format!(
                "NetCDF: все {} годных величин файла пусты в этой грануле: {}",
                skipped.len(),
                listed(&skipped)
            ),
        }),
    }
}

/// Привязка из отдельного файла координат — там, где растр её не несёт.
///
/// Так упакован Sentinel-3: измерение лежит в одном `.nc`, а широта с долготой
/// — в соседнем, и какой это файл, говорит провайдер (раскладку продукта знает
/// только он). Здесь остаётся прочитать записанное: пара плоскостей `latitude`
/// и `longitude` одной формы — по единицам, тем же правилом, что и внутри
/// растра (см. [`northing`]).
///
/// Координатная сетка бывает разрежена: у OLCI опорная сетка вшестнадцатеро
/// у́же снимка по столбцам, у SLSTR решётка `tx` вдобавок шире его с обеих
/// сторон. Чем сетка связана с растром, спрашивается у файла — см.
/// [`seating`]; не сказал — привязки не выйдет.
///
/// Отказ — это «привязки не вышло», а не «растр плох»: снимок ляжет тем, что
/// сказал о нём каталог.
pub fn geolocation(
    resource_id: u64,
    len: u64,
    raster: Option<Frame>,
    width: u32,
    height: u32,
) -> Result<Vec<Tie>, String> {
    if width < 2 || height < 2 {
        return Err(format!("растр {}×{} мельче узла привязки", width, height));
    }
    if len > TIES_BUDGET {
        return Err(format!(
            "{} МБ на одни координаты — больше потолка привязки в {} МБ",
            len / (1024 * 1024),
            TIES_BUDGET / (1024 * 1024)
        ));
    }
    let mut reader = veldsdk::ResourceReader::new(resource_id, len);
    let mut bytes = Vec::with_capacity(len as usize);
    reader.read_to_end(&mut bytes).map_err(|e| format!("чтение: {}", e))?;
    let file = File::from_bytes(bytes).map_err(|e| format!("{}", e))?;
    let items = survey(&file)?;

    // Плоскость координат в файле не одна: рядом с широтой лежит высота, а у
    // SLSTR — ещё и координаты соседних сеток. Берётся самая подробная пара
    // одной формы: узлов у решётки два десятка, и чем гуще сетка, тем ближе
    // они к тому месту, о котором спрашивают.
    // Единственность обязательна — то же правило, что и внутри растра
    // (см. [`swath`]): две широты одной формы это уже вопрос «которая», а
    // ответа на него у файла нет.
    let pair = |pick: fn(&Item) -> bool, plane: (u32, u32)| -> Option<&Item> {
        let mut found = items.iter().filter(|item| item.plane == Some(plane) && pick(item));
        let one = found.next()?;
        found.next().is_none().then_some(one)
    };
    let mut planes: Vec<(u32, u32)> = items
        .iter()
        .filter(|item| northing(item))
        .filter_map(|item| item.plane)
        .collect();
    planes.sort_unstable_by_key(|plane| std::cmp::Reverse(u64::from(plane.0) * u64::from(plane.1)));
    planes.dedup();

    let found = planes
        .into_iter()
        .find_map(|plane| Some((plane, pair(northing, plane)?, pair(easting, plane)?)));
    let Some(((geo_h, geo_w), lat, lon)) = found else {
        return Err(format!(
            "в файле нет пары широта—долгота одной формы: {}",
            listed(&items.iter().map(|item| item.path.clone()).collect::<Vec<String>>())
        ));
    };
    if geo_w < 2 || geo_h < 2 {
        return Err(format!("координатная сетка {}×{} — не сетка", geo_w, geo_h));
    }
    // Сетка гуще растра — значит она не от него: у поотсчётного файла она
    // ровно его размера, у опорного реже. Растянуть по такой значит уложить
    // снимок мимо себя, и молча.
    if geo_w > width || geo_h > height {
        return Err(format!(
            "координатная сетка {}×{} гуще растра {}×{} — это координаты не его",
            geo_w, geo_h, width, height
        ));
    }
    // Потолок меряется развёрнутыми отсчётами, а не длиной файла: он сжат, и
    // тем же потолком меряет свои решётки `swath` — иначе одна и та же пара
    // плоскостей проходила бы здесь и отвергалась там.
    let unpacked_size = u64::from(geo_w) * u64::from(geo_h) * 4 * 2;
    if unpacked_size > TIES_BUDGET {
        return Err(format!(
            "решётки координат {}×{} не влезают в бюджет привязки ({} МБ)",
            geo_w, geo_h, TIES_BUDGET / (1024 * 1024)
        ));
    }

    let read = |item: &Item| {
        file.dataset(&item.path)
            .and_then(|dataset| dataset.read_f32())
            .map(|values| unpacked(item, values))
            .map_err(|e| format!("чтение '{}': {}", item.path, e))
    };
    let (lat_values, lon_values) = (read(lat)?, read(lon)?);

    let attrs = globals(&file);
    let seat = seating(raster, frame(&attrs), subsampling(&attrs), (width, height), (geo_w, geo_h))?;
    let at = |row: u32, column: u32| -> Option<(f64, f64)> {
        let index = (row as usize) * (geo_w as usize) + (column as usize);
        Some((f64::from(*lat_values.get(index)?), f64::from(*lon_values.get(index)?)))
    };
    let ties = lattice(&nodes(geo_h), &nodes(geo_w), seat, at);
    if ties.is_empty() {
        return Err(format!("узлы сетки {}×{} не годятся в привязку", geo_w, geo_h));
    }
    veldsdk::log::debug!(target: "decode",
        "NetCDF привязка из соседнего файла: сетка {}×{} ('{}' и '{}'), шаг {:.2}×{:.2} пикселя от {:+.1}×{:+.1}, узлов {}",
        geo_w, geo_h, lat.path, lon.path,
        seat.step.0, seat.step.1, seat.origin.0, seat.origin.1, ties.len());
    Ok(ties)
}

/// Есть ли на что смотреть в отсчётах величины.
#[derive(PartialEq, Debug)]
enum Spread {
    /// Ни одного годного отсчёта: над этим местом величина не измерялась.
    Empty,
    /// Годные есть, но все до одного равны: показать можно, узнать нечего.
    Flat,
    /// Есть перепад — это изображение.
    Varying,
}

fn spread(values: &[f32], fill: Option<f32>) -> Spread {
    let mut first = None;
    for value in values.iter().copied().filter(|value| radiometry::is_data(*value, fill)) {
        match first {
            None => first = Some(value),
            Some(seen) if seen != value => return Spread::Varying,
            Some(_) => {}
        }
    }
    match first.is_some() {
        true => Spread::Flat,
        false => Spread::Empty,
    }
}

/// Собрать описание выбранной величины и сказать в журнале, чем выбор кончился.
fn told(
    file: &File,
    surveyed: &[Item],
    chosen: &Item,
    values: Vec<f32>,
    order: &[&Item],
    skipped: &[String],
) -> Result<Info, String> {
    let (height, width) = chosen.plane.ok_or_else(|| explain(surveyed))?;
    veldsdk::log::debug!(target: "decode",
        "NetCDF: показывается '{}' ({}, единицы '{}') — {}×{}, {} из {} величин годятся{}",
        chosen.path, chosen.said, chosen.units, width, height, order.len(), surveyed.len(),
        match skipped.is_empty() {
            true => String::new(),
            false => format!("; пропущено: {} ({})", skipped.len(), listed(skipped)),
        });

    let ties = ties(file, surveyed, chosen);
    Ok(Info {
        width,
        height,
        kind: Kind::Netcdf(Box::new(Source {
            values,
            path: chosen.path.clone(),
            fill: chosen.fill,
            said: chosen.said.clone(),
        })),
        finest: 0,
        ties,
        // Координаты NetCDF записаны в градусах и решёткой: проекции здесь не
        // бывает вовсе.
        placement: None,
        frame: frame(&globals(file)),
    })
}

/// Перечисление для одной строки журнала: первые три и «и ещё N».
/// Полный список бывает в полсотни имён, а в подписи слоя ему места нет.
fn listed(names: &[String]) -> String {
    let head = names.iter().take(3).cloned().collect::<Vec<String>>().join(", ");
    match names.len() > 3 {
        true => format!("{} и ещё {}", head, names.len() - 3),
        false => head,
    }
}

pub fn produce(info: &Info, source: &Source, emit: Emit) -> Result<(), String> {
    let values = &source.values;
    let expected = (info.width as usize) * (info.height as usize);
    if values.len() != expected {
        return Err(format!(
            "NetCDF: у '{}' {} отсчётов вместо {}×{}",
            source.path, values.len(), info.width, info.height
        ));
    }
    let mapping = mapping(&source.path, values, source.fill, info.width as usize);

    veldsdk::log::debug!(target: "decode",
        "NetCDF проход: '{}' ({}), {}×{}", source.path, source.said, info.width, info.height);

    let width = info.width as usize;
    let mut cascade = Cascade::new(0, info.width, info.height);
    let mut top = 0u32;
    while top < info.height {
        // Полоса ровно в тайл: границы полос каскада стоят там же, и лишнего
        // деления внутри него не случается.
        let rows = TILE.min(info.height - top);
        let from = (top as usize) * width;
        let slice = &values[from..from + (rows as usize) * width];
        let rgba = mapping.rgba(&Samples::F32(slice), Pixel::named(1), slice.len());
        cascade.push_rows(&rgba, rows, emit)?;
        top += rows;
    }
    cascade.finish(emit)
}

/// Разупакованные координаты: `значение · scale_factor + add_offset`.
///
/// Показываемой величине эти коэффициенты не нужны (см. [`plane`]), а
/// координатам нужны обязательно: широта у Sentinel-3 записана целыми с шагом
/// в миллионную долю градуса, и без разупаковки это не градусы, а десятки
/// миллионов — то есть не привязка, а её отсутствие.
fn unpacked(item: &Item, values: Vec<f32>) -> Vec<f32> {
    let (scale, offset) = item.packing;
    if scale == 1.0 && offset == 0.0 {
        return values;
    }
    values.into_iter().map(|value| (f64::from(value) * scale + offset) as f32).collect()
}

/// Отсчёты показываемой величины, развёрнутые в f32.
///
/// Через f32, а не по сырым байтам: порядок байтов в файле свой у каждой
/// машины-писателя, и разбирает его крейт. Коэффициенты `scale_factor` и
/// `add_offset` не применяются намеренно — преобразование это линейное и
/// возрастающее, а растяг считается по перцентилям тех же значений: в яркость
/// оно не вносит ничего, зато «нет данных» сравнивается с сырым значением, как
/// оно и записано. Координатам они, наоборот, нужны — см. [`unpacked`].
fn plane(file: &File, path: &str, width: u32, height: u32) -> Result<Vec<f32>, String> {
    let dataset = file.dataset(path).map_err(|e| format!("NetCDF: {}: {}", path, e))?;
    let pixels = u64::from(width) * u64::from(height);
    let element = width_of(&dataset.dtype().map_err(|e| format!("NetCDF: {}", e))?).unwrap_or(8);
    let needed = pixels.saturating_mul(u64::from(element) + 4);
    if needed > PLANE_BUDGET {
        return Err(format!(
            "NetCDF {}×{}: величина в памяти займёт {} МБ при потолке {} МБ",
            width,
            height,
            needed / (1024 * 1024),
            PLANE_BUDGET / (1024 * 1024)
        ));
    }
    dataset.read_f32().map_err(|e| format!("NetCDF: {}: {}", path, e))
}

/// Шаг выборки растяга по развёрнутой в строку плоскости.
///
/// Взаимно простой с шириной: отсчёты лежат построчно, и шаг, у которого с
/// шириной есть общий делитель, каждую строку попадает в одни и те же столбцы.
/// Полоса съёмки этим и опасна — у неё край полосы не измерен вовсе, и выборка
/// из одних краевых столбцов дала бы растяг по «нет данных», то есть кадр,
/// растянутый мимо своих значений. Шаг увеличивается, а не уменьшается:
/// выборка от этого только реже потолка, а реже — это дешевле.
fn sampling_step(len: usize, width: usize) -> usize {
    let mut stride = (len / STRETCH_SAMPLES).max(1);
    // Ширина ноль или один — разводить не с чем: столбец всего один.
    if width < 2 {
        return stride;
    }
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    // Шаг не длиннее самой плоскости: иначе выборка окажется пустой.
    while gcd(stride, width) != 1 && stride < len {
        stride += 1;
    }
    stride
}

/// Растяг показа по выборке значений: те же перцентили, что у широких
/// TIFF-сэмплов. «Нет данных» в выборку не идёт — иначе метка −9999 утянула бы
/// нижний край и весь снимок вышел бы белым.
///
/// `width` — ширина растра; по ней шаг выборки разводится со строкой (см.
/// [`sampling_step`]).
fn mapping(path: &str, values: &[f32], fill: Option<f32>, width: usize) -> Mapping {
    let stride = sampling_step(values.len(), width);
    let taken = values.iter().step_by(stride).count();
    let mut sample: Vec<f32> = values
        .iter()
        .step_by(stride)
        .copied()
        .filter(|value| radiometry::is_data(*value, fill))
        .collect();
    let stretch = percentile_stretch(&mut sample);

    // Числа, по которым кадр вышел таким, а не другим. Без них «белый
    // прямоугольник» на шаре объясняется только догадками: не видно ни того,
    // сколько отсчётов оказалось «нет данных», ни того, во что растянулись
    // остальные.
    veldsdk::log::debug!(target: "decode",
        "NetCDF растяг: '{}' — годных {} из {} в выборке, «нет данных» {:?}, растяг {:?}",
        path, sample.len(), taken, fill, stretch);

    match stretch {
        Some((lo, hi)) => Mapping::stretched(lo, hi, fill),
        // Ни одного годного значения в выборке. Величина при этом не пуста —
        // пустую отсеяло описание (см. [`spread`]), — значит выборка просто
        // прошла мимо годных: они реже её шага. Растягивать не по чему, и
        // выдумывать предел нельзя: любой назначенный белит всё, что выше
        // него. Значения принимаются за байты — то же правило, что у TIFF.
        None => Mapping::identity(fill),
    }
}

// ── Что показывать ─────────────────────────────────────────────

/// Величина файла вместе со всем, что о ней надо знать, чтобы решить, она ли
/// это. Собирается один раз обходом — заголовки читаются заново дороже, чем
/// хранятся.
struct Item {
    path: String,
    /// Группа, в которой величина лежит: в ней же ищутся её координаты.
    group: String,
    /// Сколько групп до корня. Подробности расчёта лежат глубже самой
    /// величины — см. [`preferred`].
    depth: usize,
    /// Форма без единичных осей: `Some((строк, столбцов))` только у плоскости.
    plane: Option<(u32, u32)>,
    /// Длина, если это одномерный ряд, — по ней узнаётся ось сетки.
    line: Option<u32>,
    /// Величина с плавающей точкой.
    real: bool,
    /// Измеряется в угловых градусах — см. [`angular`].
    angular: bool,
    /// Годится в показываемые — см. [`preferred`].
    candidate: bool,
    fill: Option<f32>,
    /// Упаковка величины: `scale_factor` и `add_offset`, как их записал файл.
    /// Единица и ноль — величина записана как есть.
    ///
    /// Показу они не нужны (см. [`plane`]), а координатам нужны обязательно:
    /// широта пакуется целыми с шагом в миллионную долю градуса, и без
    /// разупаковки это не градусы, а десятки миллионов.
    packing: (f64, f64),
    /// `long_name` или `standard_name`, если файл их назвал.
    said: String,
    /// Что записано в `units`, приведённое к нижнему регистру.
    units: String,
    /// Имена из `coordinates`, разрешённые в пути файла.
    coordinates: Vec<String>,
    /// Имена из `ancillary_variables`, разрешённые в пути файла.
    ancillary: Vec<String>,
}

/// Обход файла: все величины с их заголовками.
fn survey(file: &File) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    walk(file, "", 0, &mut items)?;
    if items.is_empty() {
        return Err("NetCDF: в файле нет ни одной величины".to_string());
    }

    // Исключаемое видно только целиком: величина попадает под исключение не
    // своим заголовком, а тем, что на неё сослалась другая. Координаты — это
    // «где», а не «что»; вспомогательные (`ancillary_variables`) — точность,
    // качество, время наблюдения: они описывают измерение, а не являются им.
    let mut named: HashSet<&str> = HashSet::new();
    for item in &items {
        named.extend(item.coordinates.iter().map(String::as_str));
        named.extend(item.ancillary.iter().map(String::as_str));
    }
    let named: HashSet<String> = named.into_iter().map(str::to_string).collect();
    for item in &mut items {
        // «Где» и «когда» — не «что», даже когда на них никто не сослался:
        // полоса съёмки Sentinel-5P держит широту с долготой плоскостями рядом
        // с измерениями, а гранула Sentinel-3 — ещё и время съёмки поотсчётно.
        // Без этого правила показывалась бы лестница градусов или лестница
        // микросекунд, и обе — поперёк полосы.
        let bearings = northing(item) || easting(item) || timing(item);
        item.candidate = item.plane.is_some() && !bearings && !named.contains(&item.path);
    }
    Ok(items)
}

fn walk(file: &File, path: &str, depth: usize, items: &mut Vec<Item>) -> Result<(), String> {
    if depth > MAX_DEPTH || items.len() >= MAX_DATASETS {
        return Ok(());
    }
    let group = match path.is_empty() {
        true => file.root(),
        false => file.group(path).map_err(|e| format!("NetCDF: {}: {}", path, e))?,
    };
    for name in group.datasets().map_err(|e| format!("NetCDF: {}", e))? {
        if items.len() >= MAX_DATASETS {
            break;
        }
        let full = format!("{}/{}", path, name);
        if let Some(item) = describe_item(file, &full, path, depth) {
            items.push(item);
        }
    }
    for name in group.groups().map_err(|e| format!("NetCDF: {}", e))? {
        walk(file, &format!("{}/{}", path, name), depth + 1, items)?;
    }
    Ok(())
}

fn describe_item(file: &File, full: &str, group: &str, depth: usize) -> Option<Item> {
    let dataset = file.dataset(full).ok()?;
    let shape = dataset.shape().ok()?;
    let dtype = dataset.dtype().ok()?;
    let real = matches!(dtype, DType::F32 | DType::F64);
    let numeric = width_of(&dtype).is_some();
    let attrs = dataset.attrs().unwrap_or_default();

    // Ось координатной сетки — это ряд, а не плоскость; величина — плоскость.
    // Единичные оси отбрасываются: время у гранулы Sentinel-5P записано осью
    // длины один, и без этого всякая её величина была бы трёхмерной.
    let sides: Vec<u64> = shape.iter().copied().filter(|side| *side > 1).collect();
    let plane = match sides.as_slice() {
        [rows, columns] if numeric => Some((fit(*rows)?, fit(*columns)?)),
        _ => None,
    };
    let line = match sides.as_slice() {
        [length] if numeric => Some(fit(*length)?),
        _ => None,
    };

    // Величина с `flag_values` — это код состояния, а не измерение: сплошная
    // лесенка из «море», «облако», «не обработано». Растянуть её в яркость
    // можно, и получится обман.
    let coded = attrs.contains_key("flag_values") || attrs.contains_key("flag_masks");

    let said = match text(&attrs, "long_name") {
        said if !said.is_empty() => said,
        _ => text(&attrs, "standard_name"),
    };
    let coordinates =
        words(text(&attrs, "coordinates")).map(|name| resolve(group, &name)).collect();
    let ancillary = words(text(&attrs, "ancillary_variables"))
        .map(|name| resolve(group, &name))
        .collect();

    Some(Item {
        path: full.to_string(),
        group: group.to_string(),
        depth,
        plane: plane.filter(|_| !coded),
        line,
        real,
        angular: angular(&text(&attrs, "units").to_ascii_lowercase()),
        candidate: false,
        fill: attrs.get("_FillValue").and_then(number),
        packing: (
            attrs.get("scale_factor").and_then(number).map_or(1.0, f64::from),
            attrs.get("add_offset").and_then(number).map_or(0.0, f64::from),
        ),
        said,
        units: text(&attrs, "units").to_ascii_lowercase(),
        coordinates,
        ancillary,
    })
}

/// Какую из годных величин показывать.
///
/// Порядок вопросов — от «что это за файл» к «что в нём главное». Ближе к
/// корню: подробности расчёта лежат в подгруппах (`SUPPORT_DATA`,
/// `DETAILED_RESULTS`), а сам продукт — снаружи. С плавающей точкой прежде
/// целой: целое в CF — это счётчик, код или индекс, а измерение почти всегда
/// дробное. Дальше по алфавиту, и это не выбор, а определённость: файл больше
/// ничего о старшинстве не говорит, а показывать всякий раз другое — хуже, чем
/// показывать всегда одно.
///
/// Список, а не один ответ: годность величины видна только по её отсчётам, а
/// они читаются (см. [`describe`]). Пустая в этой грануле величина — не
/// измерение этой гранулы, и следующий по порядку кандидат честнее её.
fn preferred(items: &[Item]) -> Vec<&Item> {
    let mut order: Vec<&Item> = items.iter().filter(|item| item.candidate).collect();
    order.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then(right.real.cmp(&left.real))
            .then(left.angular.cmp(&right.angular))
            .then(left.path.cmp(&right.path))
    });
    order
}

/// Почему показывать нечего — теми же словами, какими решали.
fn explain(items: &[Item]) -> String {
    let planes = items.iter().filter(|item| item.plane.is_some()).count();
    match planes {
        0 => format!(
            "NetCDF: среди {} величин файла нет ни одной двумерной — \
             это измерения, а не изображение",
            items.len()
        ),
        _ => format!(
            "NetCDF: все {} двумерных величин файла — координаты, коды состояния \
             или точности: измерять по ним нечего",
            planes
        ),
    }
}

// ── Где это лежит на Земле ─────────────────────────────────────

/// Решётка опорных точек показываемой величины.
///
/// Двумя путями, и оба — CF. Полоса съёмки несёт широту и долготу поотсчётно, и
/// величина называет их сама (`coordinates`). Регулярная сетка вместо этого
/// несёт две оси — ряды широт и долгот, — и узнаются они по единицам измерения
/// и длине: ось строк ровно такой длины, сколько в растре строк.
///
/// Пусто — привязки в файле не нашлось; снимок ляжет по контуру каталога.
fn ties(file: &File, items: &[Item], chosen: &Item) -> Vec<Tie> {
    let (height, width) = match chosen.plane {
        Some(plane) => plane,
        None => return Vec::new(),
    };
    if width < 2 || height < 2 {
        return Vec::new();
    }
    let (rows, columns) = (nodes(height), nodes(width));

    if let Some((lat, lon)) = swath(file, items, chosen) {
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            let index = (row as usize) * (width as usize) + (column as usize);
            Some((f64::from(*lat.get(index)?), f64::from(*lon.get(index)?)))
        };
        return lattice(&rows, &columns, Seating::SAME, at);
    }
    if let Some((lat, lon)) = grid(items, chosen, file) {
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            Some((*lat.get(row as usize)?, *lon.get(column as usize)?))
        };
        return lattice(&rows, &columns, Seating::SAME, at);
    }
    Vec::new()
}

/// Отсчёт прибора, в котором записана решётка файла: с какого её узла файл
/// начинается и сколько метров в ячейке.
///
/// Sentinel-3 объявляет это глобальными атрибутами: `track_offset` и
/// `start_offset` — номера узлов общей решётки съёмки, попавшие в первый
/// столбец и в первую строку файла, `resolution` — метры поперёк трека и вдоль
/// него. Растр и его поотсчётные координаты стоят в одном отсчёте, а опорная
/// сетка — в своём, и без этих чисел одно с другим не сходится (см.
/// [`seating`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Номер узла общей решётки, стоящего в первом столбце файла.
    pub across_at: f64,
    /// Номер узла, стоящего в первой строке.
    pub along_at: f64,
    /// Метры в ячейке поперёк трека.
    pub across: f64,
    /// Метры в ячейке вдоль трека.
    pub along: f64,
}

/// Посадка координатной сетки на растр: сколько пикселей приходится на узел и
/// на каком пикселе стои́т нулевой узел.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seating {
    step: (f64, f64),
    origin: (f64, f64),
}

impl Seating {
    /// Отсчёт в отсчёт: узел и есть пиксель.
    const SAME: Seating = Seating { step: (1.0, 1.0), origin: (0.0, 0.0) };
}

/// Глобальные атрибуты файла — те, что записаны у корневой группы. Ими
/// Sentinel-3 говорит о решётке, на которой лежит всё содержимое.
fn globals(file: &File) -> HashMap<String, AttrValue> {
    file.root().attrs().unwrap_or_default()
}

/// Отсчёт прибора из глобальных атрибутов. `None` — упаковка о нём не
/// говорит, и выводить его не из чего.
fn frame(attrs: &HashMap<String, AttrValue>) -> Option<Frame> {
    let (across, along) = resolution(&text(attrs, "resolution"))?;
    Some(Frame {
        across_at: f64::from(number(attrs.get("track_offset")?)?),
        along_at: f64::from(number(attrs.get("start_offset")?)?),
        across,
        along,
    })
}

/// Подвыборка опорной сетки — второй словарь, которым продукт называет ту же
/// связь: сколько пикселей растра приходится на узел поперёк трека и вдоль.
fn subsampling(attrs: &HashMap<String, AttrValue>) -> Option<(f64, f64)> {
    let read = |name: &str| Some(f64::from(number(attrs.get(name)?)?));
    Some((read("ac_subsampling_factor")?, read("al_subsampling_factor")?))
}

/// `resolution` записан строкой: `[ 16000 1000 ]` — поперёк трека и вдоль него.
///
/// Число вынимается целиком и обязано разобраться целиком: дробное, показанное
/// степенью, со знаком — всё это законные записи числа, а `1.2.3` не число
/// вовсе. Резать строку по цифрам нельзя: `1.5` распалось бы на единицу и
/// пятёрку, и обе прошли бы дальше как правдоподобные разрешения.
fn resolution(said: &str) -> Option<(f64, f64)> {
    let mut numbers = said
        .split(|sign: char| !matches!(sign, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
        .filter(|word| !word.is_empty())
        .map(str::parse::<f64>);
    let (across, along) = (numbers.next()?.ok()?, numbers.next()?.ok()?);
    let sane = |side: f64| side.is_finite() && side > 0.0;
    (sane(across) && sane(along)).then_some((across, along))
}

/// Как сетка `geo_w`×`geo_h` садится на растр `width`×`height`.
///
/// Порядок ответов — от прочитанного к выведенному, и он обязателен.
///
/// **Отсчёт прибора, названный обоими файлами**, отвечает первым: шаг —
/// отношение разрешений, начало — смещение растра минус смещение сетки, взятое
/// тем же шагом. Так говорит о себе SLSTR, и только так его опорная решётка
/// `tx` садится куда следует: она шире снимка с обеих сторон, и начало уезжает
/// то на 26 столбцов влево (сетка `in`), то на 52 (полукилометровая `an`), то
/// на 574 (косой обзор `fo`).
///
/// **Сетка размером с растр** садится отсчёт в отсчёт. Это вывод, а не
/// прочитанное, и потому вторым ответом: отсчёт прибора, если он назван, знает
/// точнее.
///
/// **Подвыборка** — последней, и обязана сойтись с размерами.
/// `ac_subsampling_factor` описывает опорную сетку *продукта*, а не тот файл, в
/// котором записан: у OLCI его несёт и поотсчётный `geo_coordinates.nc`.
/// Спрошенная раньше размеров, она растянула бы поотсчётную сетку в
/// шестнадцать раз.
///
/// **Ничего не сказано — отказ.** Посадка, выведенная из одних размеров, молчит
/// ровно там, где ошибается: крайний узел решётки `tx` приходится на столбец
/// 2038 растра шириной 1500, и такой вывод растянул бы снимок поперёк трека в
/// 1,38 раза. По контуру каталога он ляжет хотя бы примерно верно.
fn seating(
    raster: Option<Frame>,
    grid: Option<Frame>,
    subsampled: Option<(f64, f64)>,
    (width, height): (u32, u32),
    (geo_w, geo_h): (u32, u32),
) -> Result<Seating, String> {
    if let (Some(raster), Some(grid)) = (raster, grid) {
        let step = (grid.across / raster.across, grid.along / raster.along);
        return Ok(Seating {
            step,
            origin: (
                raster.across_at - step.0 * grid.across_at,
                raster.along_at - step.1 * grid.along_at,
            ),
        });
    }
    if (geo_w, geo_h) == (width, height) {
        return Ok(Seating::SAME);
    }
    if let Some((across, along)) = subsampled {
        // Сойтись с размерами обязано с точностью до пикселя: разошедшееся
        // здесь значит, что подвыборка не про эту пару, и посадка вышла бы
        // такой же молчаливой, как выведенная из размеров.
        let spans = |step: f64, nodes: u32, side: u32| {
            step > 0.0 && (f64::from(nodes - 1) * step - f64::from(side - 1)).abs() < 1.0
        };
        if spans(across, geo_w, width) && spans(along, geo_h, height) {
            return Ok(Seating { step: (across, along), origin: (0.0, 0.0) });
        }
        return Err(format!(
            "подвыборка {}×{} не сходится с сеткой {}×{} на растре {}×{}",
            across, along, geo_w, geo_h, width, height
        ));
    }
    Err(format!(
        "сетка {}×{} реже растра {}×{}, а чем она с ним связана, файл не говорит",
        geo_w, geo_h, width, height
    ))
}

/// Узлы по одной оси: индексы отсчётов, на которых стоят опорные точки.
fn nodes(side: u32) -> Vec<u32> {
    let count = TIE_GRID.min(side);
    (0..count)
        .map(|at| {
            let last = f64::from(side - 1);
            (f64::from(at) * last / f64::from(count - 1)).round() as u32
        })
        .collect()
}

/// Решётка из узлов. Одна негодная точка отменяет всю привязку: у полосы съёмки
/// широта бывает не заполнена по краям, а решётка с дырой уложила бы снимок
/// куда попало — по контуру каталога он ляжет хотя бы примерно верно.
///
/// Узел стои́т в середине отсчёта (+0.5): широта записана для центра пикселя, а
/// не для его угла.
///
/// `seat` — как узлы сетки садятся на пиксели растра (см. [`seating`]).
fn lattice(
    rows: &[u32],
    columns: &[u32],
    seat: Seating,
    at: impl Fn(u32, u32) -> Option<(f64, f64)>,
) -> Vec<Tie> {
    let mut ties = Vec::with_capacity(rows.len() * columns.len());
    for &row in rows {
        for &column in columns {
            let Some((lat, lon)) = at(row, column) else { return Vec::new() };
            // Долгота проверяется так же, как широта: незаполненный отсчёт
            // помечен не только `NaN`, но и числом (9.96921e36 у CF), и
            // проверенная лишь по широте дыра прошла бы долготой. Круг здесь
            // полный с запасом: файлы пишут долготу и как −180…180, и как
            // 0…360, а развернёт её потребитель (см. `Grid::unwind`).
            let placed = (-90.0..=90.0).contains(&lat) && (-360.0..=360.0).contains(&lon);
            if !lat.is_finite() || !lon.is_finite() || !placed {
                return Vec::new();
            }
            ties.push(Tie {
                px: f64::from(column) * seat.step.0 + seat.origin.0 + 0.5,
                py: f64::from(row) * seat.step.1 + seat.origin.1 + 0.5,
                lat,
                lon,
            });
        }
    }
    ties
}

/// Поотсчётные широта и долгота полосы съёмки — те, которые величина назвала
/// в `coordinates`.
fn swath(file: &File, items: &[Item], chosen: &Item) -> Option<(Vec<f32>, Vec<f32>)> {
    let plane = chosen.plane?;
    let named: Vec<&Item> = chosen
        .coordinates
        .iter()
        .filter_map(|path| items.iter().find(|item| &item.path == path))
        .filter(|item| item.plane == chosen.plane)
        .collect();
    // Названное `coordinates` — первый ответ: файл сам сказал, где лежат его
    // отсчёты, и спорить тут не с чем. Не сказал — спрашиваем единицы:
    // плоскость в `degrees_north` той же формы и той же группы и есть широта
    // этого измерения. Так лежит `.nc` внутри `.SAFE`: упаковка там своя, и
    // CF-атрибута `coordinates` у измерений нет вовсе, а широта с долготой
    // лежат рядом и названы единицами.
    //
    // Единственность обязательна: две широты одной формы — это уже вопрос
    // «которая», а ответа на него у файла нет. Гранула SLSTR держит их
    // несколько (`latitude_in`, `latitude_tx`), но разной формы, и под это
    // условие они не подпадают.
    let alone = |pick: fn(&Item) -> bool| -> Option<&Item> {
        let mut found = items
            .iter()
            .filter(|item| item.plane == chosen.plane && item.group == chosen.group && pick(item));
        let one = found.next()?;
        found.next().is_none().then_some(one)
    };
    let lat = match named.iter().copied().find(|item| northing(item)) {
        Some(lat) => lat,
        None => alone(northing)?,
    };
    let lon = match named.iter().copied().find(|item| easting(item)) {
        Some(lon) => lon,
        None => alone(easting)?,
    };
    // Бюджет проверяется здесь, а не в начале: у регулярной сетки поотсчётных
    // координат нет вовсе, и жаловаться на их размер было бы не про неё.
    let pixels = u64::from(plane.0) * u64::from(plane.1);
    if pixels.saturating_mul(4 * 2) > TIES_BUDGET {
        veldsdk::log::debug!(target: "decode",
            "NetCDF: решётки координат {}×{} не влезают в бюджет привязки", plane.1, plane.0);
        return None;
    }
    let read =
        |item: &Item| Some(unpacked(item, file.dataset(&item.path).ok()?.read_f32().ok()?));
    Some((read(lat)?, read(lon)?))
}

/// Оси регулярной сетки: ряды широт и долгот той же длины, что стороны растра.
/// Ищутся в группе величины и в корне — там их и держит CF.
fn grid(items: &[Item], chosen: &Item, file: &File) -> Option<(Vec<f64>, Vec<f64>)> {
    let (height, width) = chosen.plane?;
    let nearby = |item: &Item| item.group == chosen.group || item.group.is_empty();
    let lat =
        items.iter().find(|item| nearby(item) && item.line == Some(height) && northing(item))?;
    let lon =
        items.iter().find(|item| nearby(item) && item.line == Some(width) && easting(item))?;
    let read = |item: &Item| {
        let values = file.dataset(&item.path).ok()?.read_f64().ok()?;
        let (scale, offset) = item.packing;
        Some(values.into_iter().map(|value| value * scale + offset).collect::<Vec<f64>>())
    };
    Some((read(lat)?, read(lon)?))
}

/// Время съёмки — «когда», а не «что», и правило то же, что у широты с
/// долготой: плоскость времени лежит рядом с измерениями, а растягивается в
/// ровную лесенку поперёк полосы.
///
/// Узнаётся по первому слову единиц. CF пишет время как «<единица> since
/// <момент>», упаковки Sentinel-3 обходятся одной единицей («microseconds»,
/// а «с какого мига» написано словами в `long_name`), — ловится и то, и
/// другое. По первому слову, а не по вхождению: «m s-1» — это скорость.
fn timing(item: &Item) -> bool {
    matches!(
        item.units.split_whitespace().next().unwrap_or_default(),
        "s" | "sec" | "secs" | "second" | "seconds"
            | "ms" | "millisecond" | "milliseconds"
            | "us" | "microsecond" | "microseconds"
            | "ns" | "nanosecond" | "nanoseconds"
            | "min" | "minute" | "minutes"
            | "h" | "hr" | "hour" | "hours"
            | "d" | "day" | "days"
            | "year" | "years"
    )
}

/// Угловая величина: направление ветра, зенит солнца, азимут наблюдения.
///
/// Яркостью такое не показывается, и дело не в красоте. Угол замкнут: 359° и
/// 1° — соседи, а растяг перцентилей разводит их по краям шкалы, и ровное поле
/// направления выходит белым с чёрным клином на месте оборота. Угол наблюдения
/// — и вовсе не измерение, а геометрия съёмки: ровная лесенка поперёк полосы.
///
/// Поэтому такая величина идёт последней среди годных (см. [`preferred`]), но
/// не выбрасывается: если ничего другого в файле нет, показать её честнее, чем
/// промолчать. Широта с долготой сюда не попадают — их отбирают раньше и по
/// своим единицам (`degrees_north`, `degrees_east`).
fn angular(units: &str) -> bool {
    matches!(units.trim(), "degrees" | "degree" | "deg" | "degrees_t" | "radians" | "rad")
}

/// Широта ли это. По единицам измерения — так CF велит их и записывать; имя
/// величины при этом бывает любым (`lat`, `latitude`, `LATITUDE`).
fn northing(item: &Item) -> bool {
    item.units.starts_with("degrees_north") || item.units.starts_with("degree_north")
}

fn easting(item: &Item) -> bool {
    item.units.starts_with("degrees_east") || item.units.starts_with("degree_east")
}

// ── Мелочь ─────────────────────────────────────────────────────

/// Байт на отсчёт; `None` — тип не числовой, показывать его нечем.
fn width_of(dtype: &DType) -> Option<u32> {
    Some(match dtype {
        DType::I8 | DType::U8 => 1,
        DType::I16 | DType::U16 => 2,
        DType::I32 | DType::U32 | DType::F32 => 4,
        DType::I64 | DType::U64 | DType::F64 => 8,
        _ => return None,
    })
}

/// Сторона растра числом. `None` — она больше, чем бывает у растра; такую
/// величину показывать всё равно нечем.
fn fit(side: u64) -> Option<u32> {
    u32::try_from(side).ok()
}

fn text(attrs: &HashMap<String, AttrValue>, name: &str) -> String {
    attrs.get(name).and_then(|value| value.as_str()).unwrap_or_default().to_string()
}

/// Число из атрибута. Спрашивается обеими лестницами нарочно: `_FillValue`
/// записан типом самой величины, и у целочисленной он целый (`-9999` у
/// температуры поверхности), а у дробной дробный — одной мало.
fn number(value: &AttrValue) -> Option<f32> {
    let single = |numbers: Vec<f64>| numbers.first().copied();
    value
        .as_f64()
        .or_else(|| value.to_f64s().and_then(single))
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.to_i64s().and_then(|numbers| numbers.first().map(|n| *n as f64)))
        .map(|number| number as f32)
}

/// Имена в атрибуте CF идут через пробел, а иногда и через запятую.
fn words(text: String) -> impl Iterator<Item = String> {
    text.split([' ', ',', '\t', '\n'])
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Имя из атрибута — в путь файла. Абсолютное берётся как есть, остальные
/// ищутся в группе самой величины: так их и понимает CF.
fn resolve(group: &str, name: &str) -> String {
    match name.starts_with('/') {
        true => name.to_string(),
        false => format!("{}/{}", group, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Узлы стоят по краям и равномерно между ними, без повторов: повтор
    /// сложил бы решётку не в прямоугольник, и привязка отвалилась бы целиком.
    #[test]
    fn nodes_span_the_side_without_repeats() {
        assert_eq!(nodes(2), vec![0, 1]);
        assert_eq!(nodes(21).len(), 21);
        let wide = nodes(450);
        assert_eq!(wide.len(), 21);
        assert_eq!(wide[0], 0);
        assert_eq!(*wide.last().unwrap(), 449);
        assert!(wide.windows(2).all(|pair| pair[0] < pair[1]), "{:?}", wide);
    }

    /// Одна негодная точка отменяет всю решётку: снимок с дырой в привязке
    /// ляжет куда попало, а по контуру каталога — хотя бы примерно верно.
    #[test]
    fn a_single_broken_node_drops_the_whole_lattice() {
        let rows = [0u32, 1];
        let columns = [0u32, 1];
        let whole = lattice(&rows, &columns, Seating::SAME, |row, column| {
            Some((f64::from(row), f64::from(column)))
        });
        assert_eq!(whole.len(), 4);
        assert_eq!(whole[0].px, 0.5, "узел стои́т в середине отсчёта");

        let holed = lattice(&rows, &columns, Seating::SAME, |row, column| match (row, column) {
            (1, 1) => Some((f64::NAN, 0.0)),
            _ => Some((f64::from(row), f64::from(column))),
        });
        assert!(holed.is_empty());

        // Широта за пределами шара — тоже дыра, только записанная числом:
        // так лежит незаполненный край полосы съёмки. И долгота тоже: у
        // решётки, проверенной лишь по широте, такая дыра проходила бы.
        let filled = lattice(&rows, &columns, Seating::SAME, |_, _| Some((9.969_21e36, 0.0)));
        assert!(filled.is_empty());
        let eastless = lattice(&rows, &columns, Seating::SAME, |_, _| Some((0.0, 9.969_21e36)));
        assert!(eastless.is_empty());

        // Опорная сетка разрежена: отсчёт координат стои́т не на каждом пикселе,
        // и узел уезжает туда, куда указывает шаг.
        let sparse = lattice(&rows, &columns, Seating { step: (64.0, 1.0), origin: (0.0, 0.0) }, |row, column| {
            Some((f64::from(row), f64::from(column)))
        });
        assert_eq!(sparse[1].px, 64.5, "второй столбец опорной сетки — 64-й пиксель");
        assert_eq!(sparse[1].py, 0.5);

        // У посадки есть и начало: решётка `tx` SLSTR свисает за левый край
        // растра, и её нулевой узел приходится на 26 пикселей левее нуля.
        let shifted = lattice(
            &rows,
            &columns,
            Seating { step: (16.0, 1.0), origin: (-26.0, 0.0) },
            |row, column| Some((f64::from(row), f64::from(column))),
        );
        assert_eq!(shifted[0].px, -25.5, "нулевой узел стои́т левее растра");
        assert_eq!(shifted[1].px, -9.5);
        assert_eq!(shifted[1].py, 0.5, "по строкам начала нет — они совпадают отсчёт в отсчёт");
    }

    /// Отсчёт прибора у гранулы SLSTR: растр `in` и решётка `tx` стоят в
    /// разных отсчётах, и сходятся они только через смещения.
    ///
    /// Числа настоящие — `S3A_SL_2_LST____20260824T174507`: растр 266×1500 при
    /// `track_offset` 998 и разрешении 1000 м, решётка 266×130 при
    /// `track_offset` 64 и разрешении 16000 м поперёк трека.
    #[test]
    fn the_slstr_tie_grid_sits_by_the_instrument_frame() {
        let raster = Frame { across_at: 998.0, along_at: 3598.0, across: 1000.0, along: 1000.0 };
        let grid = Frame { across_at: 64.0, along_at: 3598.0, across: 16000.0, along: 1000.0 };
        let seat = seating(Some(raster), Some(grid), None, (1500, 266), (130, 266))
            .expect("отсчёты названы обоими файлами");
        assert_eq!(seat.step, (16.0, 1.0), "шаг — отношение разрешений");
        assert_eq!(seat.origin, (-26.0, 0.0), "нулевой узел свисает за левый край растра");
        // Крайний узел приходится далеко за правый край — решётка шире снимка.
        assert_eq!(129.0 * seat.step.0 + seat.origin.0, 2038.0);
    }

    /// Опорная сетка OLCI названа подвыборкой, и та обязана сойтись с
    /// размерами: узлы стоят на краях растра.
    ///
    /// Числа настоящие — `S3A_OL_2_LRR____20260824T161836`: растр 15076×1217,
    /// сетка 15076×77, `ac_subsampling_factor` 16, `al_subsampling_factor` 1.
    #[test]
    fn the_olci_tie_grid_sits_by_its_subsampling() {
        let seat = seating(None, None, Some((16.0, 1.0)), (1217, 15076), (77, 15076))
            .expect("подвыборка сходится с размерами");
        assert_eq!(seat.step, (16.0, 1.0));
        assert_eq!(seat.origin, (0.0, 0.0), "узел ноль стои́т на пикселе ноль");
        assert_eq!(76.0 * seat.step.0 + seat.origin.0, 1216.0, "крайний узел — крайний столбец");
    }

    /// Сетка размером с растр садится отсчёт в отсчёт, и подвыборку у неё не
    /// спрашивают. Правило это не про дешевизну: `ac_subsampling_factor`
    /// описывает опорную сетку продукта, и поотсчётный `geo_coordinates.nc`
    /// OLCI несёт его тоже. Спрошенный раньше размеров, он растянул бы
    /// поотсчётную сетку в шестнадцать раз.
    #[test]
    fn a_grid_the_size_of_the_raster_ignores_the_subsampling_attribute() {
        let seat = seating(None, None, Some((16.0, 1.0)), (1217, 15076), (1217, 15076))
            .expect("сетка размером с растр садится всегда");
        assert_eq!(seat, Seating::SAME);
    }

    /// Подвыборка, не сошедшаяся с размерами, — это подвыборка не про эту
    /// пару. Посадка по ней вышла бы такой же молчаливой, как выведенная из
    /// размеров, поэтому здесь отказ.
    #[test]
    fn a_subsampling_that_misses_the_sizes_is_refused() {
        let off = seating(None, None, Some((16.0, 1.0)), (1217, 15076), (130, 15076));
        assert!(off.is_err(), "77 узлов ожидалось, а сетка о 130");
        // Допуск — пиксель, а не «примерно»: 77 узлов шагом 16 покрывают 1217
        // столбцов, и растру шириной 1220 та же подвыборка уже не отвечает.
        let near = seating(None, None, Some((16.0, 1.0)), (1220, 15076), (77, 15076));
        assert!(near.is_err(), "разошлись на три столбца — это уже другая пара");
    }

    /// Разрежённая сетка без единого слова о том, чем она связана с растром,
    /// не садится вовсе: снимок ляжет по контуру каталога, а не мимо себя.
    #[test]
    fn a_sparse_grid_that_says_nothing_about_itself_is_refused() {
        let mute = seating(None, None, None, (1500, 266), (130, 266));
        assert!(mute.is_err());
    }

    /// Полукилометровая сетка `an` и косой обзор `fo` — те случаи, где обе оси
    /// и оба начала разные, и симметричная описка в них была бы видна.
    ///
    /// Числа настоящие — `S3A_SL_1_RBT____20260824T174507`: у `an` растр
    /// 533×3000 при `track_offset` 1996, `start_offset` 7195 и разрешении 500 м
    /// по обеим осям; у `fo` растр 266×900 при `track_offset` 450. Решётка `tx`
    /// им обоим одна и та же.
    #[test]
    fn every_slstr_grid_seats_on_the_same_tie_lattice() {
        let tie = Frame { across_at: 64.0, along_at: 3598.0, across: 16000.0, along: 1000.0 };

        let half_kilometre = Frame { across_at: 1996.0, along_at: 7195.0, across: 500.0, along: 500.0 };
        let seat = seating(Some(half_kilometre), Some(tie), None, (3000, 533), (130, 266))
            .expect("отсчёты названы обоими файлами");
        assert_eq!(seat.step, (32.0, 2.0), "полукилометровому пикселю узел вдвое дороже");
        assert_eq!(seat.origin, (-52.0, -1.0), "начало уезжает по обеим осям");

        let oblique = Frame { across_at: 450.0, along_at: 3598.0, across: 1000.0, along: 1000.0 };
        let seat = seating(Some(oblique), Some(tie), None, (900, 266), (130, 266))
            .expect("отсчёты названы обоими файлами");
        assert_eq!(seat.step, (16.0, 1.0));
        assert_eq!(seat.origin, (-574.0, 0.0), "косой обзор смещён поперёк трека сильнее надирного");
    }

    /// Отсчёт прибора отвечает раньше размеров и раньше подвыборки: он
    /// прочитан, а те выведены.
    #[test]
    fn the_instrument_frame_outranks_both_guesses() {
        let raster = Frame { across_at: 998.0, along_at: 3598.0, across: 1000.0, along: 1000.0 };
        let shifted = Frame { across_at: 450.0, along_at: 3598.0, across: 1000.0, along: 1000.0 };

        // Формы совпадают, а отсчёты — нет: садиться надо по отсчётам.
        let seat = seating(Some(raster), Some(shifted), None, (1500, 266), (1500, 266))
            .expect("отсчёты названы обоими файлами");
        assert_eq!(seat.origin, (548.0, 0.0), "равенство форм отсчёта не отменяет");

        // Подвыборка названа тоже, и она о другом — отсчёт главнее.
        let seat = seating(Some(raster), Some(shifted), Some((4.0, 4.0)), (1500, 266), (1500, 266))
            .expect("отсчёты названы обоими файлами");
        assert_eq!(seat.step, (1.0, 1.0));
        assert_eq!(seat.origin, (548.0, 0.0));
    }

    /// Отсчёт прибора и подвыборка читаются из глобальных атрибутов, и каждая
    /// ось берётся из своего: перепутанные, они разъехались бы молча.
    #[test]
    fn the_global_attributes_are_read_axis_by_axis() {
        let attrs: HashMap<String, AttrValue> = [
            ("resolution".to_string(), AttrValue::String("[ 16000 1000 ]".to_string())),
            ("track_offset".to_string(), AttrValue::I32(64)),
            ("start_offset".to_string(), AttrValue::I32(3598)),
            ("ac_subsampling_factor".to_string(), AttrValue::I32(16)),
            ("al_subsampling_factor".to_string(), AttrValue::I32(1)),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            frame(&attrs),
            Some(Frame { across_at: 64.0, along_at: 3598.0, across: 16000.0, along: 1000.0 })
        );
        assert_eq!(subsampling(&attrs), Some((16.0, 1.0)));

        // Без единого из трёх отсчёта не выходит.
        for missing in ["resolution", "track_offset", "start_offset"] {
            let mut lean = attrs.clone();
            lean.remove(missing);
            assert_eq!(frame(&lean), None, "без '{}' отсчёт не собрать", missing);
        }
        let mut lean = attrs.clone();
        lean.remove("al_subsampling_factor");
        assert_eq!(subsampling(&lean), None, "подвыборка нужна по обеим осям");
    }

    /// Отсчёт прибора без пары бесполезен: одно смещение не говорит ни о шаге,
    /// ни о начале.
    #[test]
    fn one_instrument_frame_without_the_other_is_not_enough() {
        let raster = Frame { across_at: 998.0, along_at: 3598.0, across: 1000.0, along: 1000.0 };
        assert!(seating(Some(raster), None, None, (1500, 266), (130, 266)).is_err());
        assert!(seating(None, Some(raster), None, (1500, 266), (130, 266)).is_err());
    }

    /// `resolution` записан строкой в скобках, и читаются оба числа.
    #[test]
    fn the_resolution_attribute_is_read_out_of_its_brackets() {
        assert_eq!(resolution("[ 16000 1000 ]"), Some((16000.0, 1000.0)));
        assert_eq!(resolution("[ 1000 1000 ]"), Some((1000.0, 1000.0)));
        assert_eq!(resolution("[ 1080 1176 ]"), Some((1080.0, 1176.0)));
        assert_eq!(resolution("1000"), None, "одного числа мало");
        assert_eq!(resolution(""), None);
        assert_eq!(resolution("[ 0 1000 ]"), None, "нулевая ячейка — не разрешение");
        // Число берётся целиком: разрезанное по цифрам, `1.5` дало бы единицу и
        // пятёрку, и обе прошли бы дальше как правдоподобные разрешения.
        assert_eq!(resolution("[ 1.5 2.5 ]"), Some((1.5, 2.5)));
        assert_eq!(resolution("[ 1e3 2e3 ]"), Some((1000.0, 2000.0)));
        assert_eq!(resolution("[ -1000 -1000 ]"), None, "знак не теряется");
        assert_eq!(resolution("[ 1.2.3 1000 ]"), None, "не число — не разрешение");
    }

    /// Шаг выборки разводится со строкой: общий делитель у шага и ширины
    /// оставил бы выборку в одних и тех же столбцах каждой строки, а у полосы
    /// съёмки крайние столбцы не измерены вовсе.
    #[test]
    fn the_sampling_step_never_walks_one_column() {
        // Гранула OLCI: 4865×1687 отсчётов на выборку в миллион даёт шаг 7, а
        // 4865 = 5·7·139 — то есть исходный шаг делит ширину, и выборка ходила
        // бы по одной седьмой столбцов. Разведённый шаг — 8.
        let (width, height) = (4865usize, 1687usize);
        let step = sampling_step(width * height, width);
        assert_eq!(step, 8, "шаг разводится с шириной");
        assert_eq!(gcd_for_test(step, width), 1);

        // Плоскость мельче выборки: шаг единичный, разводить нечего.
        assert_eq!(sampling_step(450 * 372, 450), 1);

        // И общее правило — на ширинах, где шаг заведомо больше единицы.
        for width in 2..64usize {
            let step = sampling_step(width * STRETCH_SAMPLES * 3, width);
            assert!(step > 1, "шаг должен быть больше единицы: {}", step);
            assert_eq!(
                gcd_for_test(step, width), 1,
                "шаг {} и ширина {} не взаимно просты", step, width
            );
        }
    }

    fn gcd_for_test(mut a: usize, mut b: usize) -> usize {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    }

    /// Растяг считается по годным значениям: метка «нет данных» утянула бы
    /// нижний край, и весь снимок вышел бы белым.
    #[test]
    fn stretch_ignores_the_fill_value() {
        let mut values = vec![-9999.0f32; 100];
        values.extend((0..=100i16).map(|value| f32::from(value) + 300.0));
        let mapping = mapping("/проба", &values, Some(-9999.0), 1);
        // Поле «нет данных» — прозрачное, а живое значение попадает в середину
        // растяга, а не в белое.
        let out = mapping.rgba(&Samples::F32(&[-9999.0, 350.0]), Pixel::named(1), 2);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert!((100..=160).contains(&out[4]), "середина растяга: {}", out[4]);
    }

    /// Пустая величина — не изображение гранулы, однотонная — изображение
    /// последней очереди. Ловится это только по самим отсчётам: заголовок у
    /// пустой и у заполненной один и тот же.
    #[test]
    fn an_unfilled_variable_is_not_a_picture() {
        let fill = Some(65535.0);
        assert_eq!(spread(&[65535.0; 8], fill), Spread::Empty);
        assert_eq!(spread(&[], fill), Spread::Empty);
        assert_eq!(spread(&[f32::NAN, f32::INFINITY], None), Spread::Empty);
        assert_eq!(spread(&[0.0, 65535.0, 0.0], fill), Spread::Flat);
        assert_eq!(spread(&[0.0, 1.0, 65535.0], fill), Spread::Varying);
        // Без метки «нет данных» пустоту делают только нечисла.
        assert_eq!(spread(&[7.0, 7.0], None), Spread::Flat);
    }

    /// Имена координат разрешаются в пути: относительное — от группы величины,
    /// абсолютное — как есть.
    #[test]
    fn coordinate_names_become_paths() {
        assert_eq!(resolve("/PRODUCT", "latitude"), "/PRODUCT/latitude");
        assert_eq!(resolve("/PRODUCT", "/OTHER/latitude"), "/OTHER/latitude");
        assert_eq!(resolve("", "lat"), "/lat");
        let names: Vec<String> = words("Q_FLAGS, ERRORBAR_LST, TIME_DELTA".to_string()).collect();
        assert_eq!(names, ["Q_FLAGS", "ERRORBAR_LST", "TIME_DELTA"]);
    }

    /// «Нет данных» записано типом самой величины: у температуры поверхности
    /// это целое −9999, у индекса аэрозоля — дробное 9.96921e36. Прочитать
    /// одной лестницей нельзя, а потерянное «нет данных» утягивает растяг и
    /// красит весь снимок в один цвет.
    #[test]
    fn the_fill_value_is_read_whatever_its_type() {
        assert_eq!(number(&AttrValue::F64Array(vec![9.969_209_968_386_869e36])), Some(9.969_21e36));
        assert_eq!(number(&AttrValue::I64Array(vec![-9999])), Some(-9999.0));
        assert_eq!(number(&AttrValue::I32(-32767)), Some(-32767.0));
        assert_eq!(number(&AttrValue::F64(0.5)), Some(0.5));
        assert_eq!(number(&AttrValue::AsciiString("K".to_string())), None);
    }

    /// Из годных величин показывается та, что ближе к корню и дробная:
    /// подробности расчёта лежат в подгруппах, а целое в CF — код или счётчик.
    #[test]
    fn the_shallowest_measurement_wins() {
        let item = |path: &str, depth: usize, real: bool| Item {
            path: path.to_string(),
            group: String::new(),
            depth,
            plane: Some((10, 10)),
            line: None,
            real,
            angular: false,
            candidate: true,
            fill: None,
            packing: (1.0, 0.0),
            said: String::new(),
            units: String::new(),
            coordinates: Vec::new(),
            ancillary: Vec::new(),
        };
        let items = vec![
            item("/PRODUCT/SUPPORT_DATA/cloud_fraction", 2, true),
            item("/PRODUCT/qa_value", 1, false),
            item("/PRODUCT/aerosol_index_354_388", 1, true),
            item("/PRODUCT/aerosol_index_335_367", 1, true),
        ];
        // Глубина важнее дробности, дробность — важнее алфавита, алфавит —
        // последний, и он же делает выбор одним и тем же от запуска к запуску.
        let order = preferred(&items);
        // Угловая величина идёт после всех прочих той же глубины и дробности:
        // замкнутый угол в яркость не растягивается (см. `angular`).
        let angles = vec![
            Item { angular: true, ..item("/PRODUCT/aerosol_index_000", 1, true) },
            item("/PRODUCT/zzz_last_by_alphabet", 1, true),
        ];
        assert_eq!(
            preferred(&angles).iter().map(|item| item.path.as_str()).collect::<Vec<&str>>(),
            ["/PRODUCT/zzz_last_by_alphabet", "/PRODUCT/aerosol_index_000"],
        );
        assert!(angular("degrees") && angular(" deg ") && angular("radians"));
        assert!(!angular("m s-1") && !angular("degrees_north") && !angular("k"));

        // Время — «когда», а не «что»: по первому слову единиц, иначе «m s-1»
        // прочиталось бы секундами.
        let with_units = |units: &str| Item {
            units: units.to_string(),
            ..item("/x", 1, true)
        };
        assert!(timing(&with_units("microseconds")));
        assert!(timing(&with_units("seconds since 2000-01-01 00:00:00")));
        assert!(!timing(&with_units("m s-1")));
        assert!(!timing(&with_units("kelvin")));
        assert!(!timing(&with_units("")));
        assert_eq!(
            order.iter().map(|item| item.path.as_str()).collect::<Vec<&str>>(),
            [
                "/PRODUCT/aerosol_index_335_367",
                "/PRODUCT/aerosol_index_354_388",
                "/PRODUCT/qa_value",
                "/PRODUCT/SUPPORT_DATA/cloud_fraction",
            ],
        );

        // Не осталось годных — сказано, чего именно не нашлось.
        let none: Vec<Item> = items
            .into_iter()
            .map(|item| Item { candidate: false, ..item })
            .collect();
        assert!(preferred(&none).is_empty());
        assert!(explain(&none).contains("координаты"), "{}", explain(&none));
    }
}

