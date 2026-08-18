//! NetCDF-4 (он же HDF5): измеренная величина → растр.
//!
//! Файл этот не картинка, а набор именованных величин: температура
//! поверхности, содержание газа в столбе воздуха, качество измерения, ошибка
//! измерения. Показать его — значит выбрать из них ту, которая и есть
//! измерение, узнать, где каждый её отсчёт лежит на Земле, и растянуть
//! значения в яркость. Правила выбора — CF, то есть те же, которыми файл
//! описывает себя сам (см. [`choose`]); шаблонов имён миссий здесь нет.
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
use super::radiometry::{percentile_stretch, Mapping, Pixel, Samples, STRETCH_SAMPLES};
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

/// Открытый файл вместе с тем, что в нём решено показывать.
///
/// Файл держится целиком: он уже прочитан, а `produce` вызывается сразу за
/// `describe` — перечитывать его по сети второй раз значит платить дважды за
/// то же самое.
pub struct Source {
    file: File,
    /// Путь показываемой величины внутри файла.
    path: String,
    /// Чем в ней помечено «нет данных» (`_FillValue`).
    fill: Option<f32>,
    /// Как величина называется в файле по-человечески — для журнала.
    said: String,
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
    let chosen = choose(&surveyed).ok_or_else(|| explain(&surveyed))?;
    let Some((height, width)) = chosen.plane else {
        return Err(explain(&surveyed));
    };

    veldsdk::log::debug!(target: "decode",
        "NetCDF: показывается '{}' ({}) — {}×{}, {} из {} величин годятся",
        chosen.path, chosen.said, width, height,
        surveyed.iter().filter(|item| item.candidate).count(), surveyed.len());

    let ties = ties(&file, &surveyed, chosen);
    Ok(Info {
        width,
        height,
        kind: Kind::Netcdf(Box::new(Source {
            file,
            path: chosen.path.clone(),
            fill: chosen.fill,
            said: chosen.said.clone(),
        })),
        finest: 0,
        ties,
    })
}

pub fn produce(info: &Info, source: &Source, emit: Emit) -> Result<(), String> {
    let values = plane(source, info.width, info.height)?;
    let expected = (info.width as usize) * (info.height as usize);
    if values.len() != expected {
        return Err(format!(
            "NetCDF: у '{}' {} отсчётов вместо {}×{}",
            source.path, values.len(), info.width, info.height
        ));
    }
    let mapping = mapping(&values, source.fill);

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

/// Отсчёты показываемой величины, развёрнутые в f32.
///
/// Через f32, а не по сырым байтам: порядок байтов в файле свой у каждой
/// машины-писателя, и разбирает его крейт. Коэффициенты `scale_factor` и
/// `add_offset` не применяются намеренно — преобразование это линейное и
/// возрастающее, а растяг считается по перцентилям тех же значений: в яркость
/// оно не вносит ничего, зато «нет данных» сравнивается с сырым значением, как
/// оно и записано.
fn plane(source: &Source, width: u32, height: u32) -> Result<Vec<f32>, String> {
    let dataset =
        source.file.dataset(&source.path).map_err(|e| format!("NetCDF: {}: {}", source.path, e))?;
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
    dataset.read_f32().map_err(|e| format!("NetCDF: {}: {}", source.path, e))
}

/// Растяг показа по выборке значений: те же перцентили, что у широких
/// TIFF-сэмплов. «Нет данных» в выборку не идёт — иначе метка −9999 утянула бы
/// нижний край и весь снимок вышел бы белым.
fn mapping(values: &[f32], fill: Option<f32>) -> Mapping {
    let stride = (values.len() / STRETCH_SAMPLES).max(1);
    let mut sample: Vec<f32> = values
        .iter()
        .step_by(stride)
        .copied()
        .filter(|value| value.is_finite() && Some(*value) != fill)
        .collect();
    match percentile_stretch(&mut sample) {
        Some((lo, hi)) => Mapping::stretched(lo, hi, fill),
        // Ни одного годного значения — растягивать нечего, и весь кадр всё
        // равно уйдёт в прозрачность.
        None => Mapping::stretched(0.0, 1.0, fill),
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
    /// величины — см. [`choose`].
    depth: usize,
    /// Форма без единичных осей: `Some((строк, столбцов))` только у плоскости.
    plane: Option<(u32, u32)>,
    /// Длина, если это одномерный ряд, — по ней узнаётся ось сетки.
    line: Option<u32>,
    /// Величина с плавающей точкой.
    real: bool,
    /// Годится в показываемые — см. [`choose`].
    candidate: bool,
    fill: Option<f32>,
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
        // Широта и долгота — это «где», а не «что», даже когда на них никто не
        // сослался: полоса съёмки Sentinel-5P держит их плоскостями рядом с
        // измерениями, и без этого правила показывалась бы лестница градусов.
        let place = northing(item) || easting(item);
        item.candidate = item.plane.is_some() && !place && !named.contains(&item.path);
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
        candidate: false,
        fill: attrs.get("_FillValue").and_then(number),
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
fn choose(items: &[Item]) -> Option<&Item> {
    items
        .iter()
        .filter(|item| item.candidate)
        .min_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then(right.real.cmp(&left.real))
                .then(left.path.cmp(&right.path))
        })
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
        return lattice(&rows, &columns, at);
    }
    if let Some((lat, lon)) = grid(items, chosen, file) {
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            Some((*lat.get(row as usize)?, *lon.get(column as usize)?))
        };
        return lattice(&rows, &columns, at);
    }
    Vec::new()
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
fn lattice(
    rows: &[u32],
    columns: &[u32],
    at: impl Fn(u32, u32) -> Option<(f64, f64)>,
) -> Vec<Tie> {
    let mut ties = Vec::with_capacity(rows.len() * columns.len());
    for &row in rows {
        for &column in columns {
            let Some((lat, lon)) = at(row, column) else { return Vec::new() };
            if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
                return Vec::new();
            }
            ties.push(Tie {
                px: f64::from(column) + 0.5,
                py: f64::from(row) + 0.5,
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
    let lat = named.iter().copied().find(|item| northing(item))?;
    let lon = named.iter().copied().find(|item| easting(item))?;
    // Бюджет проверяется здесь, а не в начале: у регулярной сетки поотсчётных
    // координат нет вовсе, и жаловаться на их размер было бы не про неё.
    let pixels = u64::from(plane.0) * u64::from(plane.1);
    if pixels.saturating_mul(4 * 2) > TIES_BUDGET {
        veldsdk::log::debug!(target: "decode",
            "NetCDF: решётки координат {}×{} не влезают в бюджет привязки", plane.1, plane.0);
        return None;
    }
    let read = |item: &Item| file.dataset(&item.path).ok()?.read_f32().ok();
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
    let read = |item: &Item| file.dataset(&item.path).ok()?.read_f64().ok();
    Some((read(lat)?, read(lon)?))
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
        let whole = lattice(&rows, &columns, |row, column| {
            Some((f64::from(row), f64::from(column)))
        });
        assert_eq!(whole.len(), 4);
        assert_eq!(whole[0].px, 0.5, "узел стои́т в середине отсчёта");

        let holed = lattice(&rows, &columns, |row, column| match (row, column) {
            (1, 1) => Some((f64::NAN, 0.0)),
            _ => Some((f64::from(row), f64::from(column))),
        });
        assert!(holed.is_empty());

        // Широта за пределами шара — тоже дыра, только записанная числом:
        // так лежит незаполненный край полосы съёмки.
        let filled = lattice(&rows, &columns, |_, _| Some((9.969_21e36, 0.0)));
        assert!(filled.is_empty());
    }

    /// Растяг считается по годным значениям: метка «нет данных» утянула бы
    /// нижний край, и весь снимок вышел бы белым.
    #[test]
    fn stretch_ignores_the_fill_value() {
        let mut values = vec![-9999.0f32; 100];
        values.extend((0..=100i16).map(|value| f32::from(value) + 300.0));
        let mapping = mapping(&values, Some(-9999.0));
        // Поле «нет данных» — прозрачное, а живое значение попадает в середину
        // растяга, а не в белое.
        let out = mapping.rgba(&Samples::F32(&[-9999.0, 350.0]), Pixel::named(1), 2);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert!((100..=160).contains(&out[4]), "середина растяга: {}", out[4]);
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
            candidate: true,
            fill: None,
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
        assert_eq!(choose(&items).unwrap().path, "/PRODUCT/aerosol_index_335_367");

        // Не осталось годных — сказано, чего именно не нашлось.
        let none: Vec<Item> = items
            .into_iter()
            .map(|item| Item { candidate: false, ..item })
            .collect();
        assert!(choose(&none).is_none());
        assert!(explain(&none).contains("координаты"), "{}", explain(&none));
    }
}

