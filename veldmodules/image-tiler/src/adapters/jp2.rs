//! JPEG 2000 (JP2/J2C): квиклуки и гранулы Sentinel-2.
//!
//! Декодер (hayro-jpeg2000) читает кадр целиком и умеет декодировать сразу в
//! огрублённое разрешение: пропуск уровней DWT — та же ceil-лестница деления
//! пополам, что у пирамиды, поэтому декод всегда ложится на её уровень.
//! Но огрубление — лишь пожелание: палитровым файлам крейт молча его
//! выключает, пропуск не бывает глубже размеченного в файле числа уровней
//! DWT, а floor-математика выбора пропуска на нечётных размерах отстаёт на
//! ступень — декод выходит запрошенным уровнем или мельче. Каскад стартует с
//! фактического уровня: запрошенные тайлы производятся по пути, а уровни
//! мельче бонусом уезжают в кэш.
//!
//! Регионального декода у крейта нет, отсюда две границы. Память прохода
//! растёт с площадью декодируемого кадра, поэтому бюджет проверяется дважды:
//! по запрошенному уровню до чтения файла и по фактическим размерам декода
//! (`Image::width/height`, уже с учётом пропуска) до самого декода; не
//! влезает — честный отказ (у TCI 10980² это родное разрешение и половина;
//! их закроет регион в свою фазу). А описание источника не декодирует ничего:
//! размеры читаются своим разбором заголовка (`header_dims`) из первых
//! килобайт — тянуть гигабайтный файл ради describe нельзя.

use std::io::Read;

use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};

use super::super::cascade::{Cascade, Emit};
use super::super::pyramid;
use super::radiometry::Pixel;
use super::{to_rgba, Info, Kind, Metered, Placement};

/// Потолок памяти одного прохода: сам файл, f32-плоскости декодера, его
/// интерливленный выход и RGBA. Считается до чтения; уровень, не влезающий в
/// потолок, — отказ, а не смерть инстанса на пределе в 1 ГБ.
const DECODE_BUDGET: u64 = 512 * 1024 * 1024;

/// Сколько головы файла хватает заголовку: сигнатурные боксы, ftyp, jp2h
/// лежат первыми, а у сырого кодстрима SIZ — сразу за SOC.
const HEAD: usize = 64 * 1024;

pub fn describe(mut reader: Metered, len: u64) -> Result<Info, String> {
    // Длина берётся в `u64` и сравнивается там же: у модуля `usize`
    // тридцатидвухбитный, и переведи мы её первой, файл в четыре гигабайта с
    // сотней байт дал бы голову в сотню байт — а это не отказ чтения, а
    // спокойный ответ «нет сигнатуры» про совершенно годный растр.
    let mut head = vec![0u8; len.min(HEAD as u64) as usize];
    reader.read_exact(&mut head).map_err(|e| format!("jp2: чтение заголовка: {}", e))?;
    let (width, height) = header_dims(&head)?;
    // Раскладка кодстрима — не про показ, а про то, чего стои́т чтение куска:
    // единица чтения у JPEG 2000 это тайл, и нарезка решает, есть ли смысл
    // просить область вместо уровня целиком. Отказ здесь не приговор — растр
    // читается по-прежнему, просто выбирать способ не по чему.
    match codestream(&head) {
        Ok(layout) => {
            let (across, down) = layout.grid();
            veldsdk::log::debug!(target: "perf",
                "jp2 {}×{}: тайлов {}×{} по {}×{}, начало {:?}, компонент {}, разрешений {}",
                width, height, across, down, layout.tile.0, layout.tile.1,
                layout.origin, layout.components, layout.resolutions);
        }
        Err(why) => veldsdk::log::debug!(target: "perf", "jp2 {}×{}: {}", width, height, why),
    }
    let mut info = Info::plain(width, height, Kind::Jp2);
    info.finest = finest(len, width, height);
    info.placement = gml_placement(&head, width, height);
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

/// Самый подробный уровень, который влезает в бюджет декода. Считается по тем
/// же [`estimate`] и [`DECODE_BUDGET`], которыми [`produce`] потом и отказывает,
/// — затем и считается заранее: заказчику не за чем просить то, что заведомо
/// получит отказ (см. `Described.finest`).
///
/// Уровней у пирамиды конечное число, и если не влезает ни один — отдаётся
/// последний: вершина помещается в тайл, и не влезть она может только вместе с
/// самим файлом, который в память всё равно поднимать.
fn finest(len: u64, width: u32, height: u32) -> u32 {
    let top = pyramid::level_count(width, height).saturating_sub(1);
    (0..=top)
        .find(|level| {
            let (w, h) = (pyramid::level_size(width, *level), pyramid::level_size(height, *level));
            estimate(len, w, h) <= DECODE_BUDGET
        })
        .unwrap_or(top)
}

pub fn produce(
    mut reader: Metered,
    len: u64,
    info: &Info,
    level: u32,
    emit: Emit,
) -> Result<(), String> {
    let lw = pyramid::level_size(info.width, level);
    let lh = pyramid::level_size(info.height, level);
    // Быстрый отказ до чтения файла: запрошенный уровень — нижняя граница
    // памяти декода.
    let cost = estimate(len, lw, lh);
    if cost > DECODE_BUDGET {
        return Err(format!(
            "jp2: уровень {}×{} не влезает в бюджет декодера (≈{} МиБ) — возьмите уровень грубее",
            lw, lh, cost >> 20
        ));
    }

    // Файл целиком: декодеру нужен непрерывный срез. Через Metered — чтение
    // и есть та часть прохода, чей ход стоит показывать.
    let mut data = Vec::with_capacity(len as usize);
    reader.read_to_end(&mut data).map_err(|e| format!("jp2: чтение: {}", e))?;

    let settings = DecodeSettings {
        target_resolution: Some((lw, lh)),
        ..DecodeSettings::default()
    };
    let image = Image::new(&data, &settings).map_err(|e| format!("jp2: {:?}", e))?;
    match image.color_space() {
        ColorSpace::Gray | ColorSpace::RGB => {}
        // ICC-профиль не применяется — для превью числа берутся как есть.
        ColorSpace::Icc { .. } => {}
        // Кодопоток без бокса цвета — обычный JP2, а не поломка: чем считать
        // компоненты, там просто не сказано. Сколько их и сходятся ли они по
        // длине, проверяется ниже по самим компонентам, а не по имени модели.
        ColorSpace::Unknown { .. } => {}
        other => return Err(format!("jp2: цветовая модель {:?} не поддерживается", other)),
    }

    // Фактические размеры декода известны только теперь (см. заголовок файла:
    // огрубление — пожелание) — бюджет пересчитывается до самого декода.
    let (dw, dh) = (image.width(), image.height());
    let cost = estimate(len, dw, dh);
    if cost > DECODE_BUDGET {
        return Err(format!(
            "jp2: файл не даёт декодировать грубее {}×{}, а это не влезает в бюджет (≈{} МиБ)",
            dw, dh, cost >> 20
        ));
    }
    let base = ladder_level(info.width, info.height, dw, dh, level).ok_or_else(|| {
        format!("jp2: декод {}×{} мимо лестницы пирамиды {}×{}", dw, dh, info.width, info.height)
    })?;

    let mut ctx = DecoderContext::default();
    let decoded = image.decode(&mut ctx).map_err(|e| format!("jp2: {:?}", e))?;
    let components = decoded.components();
    let channels = components.len();
    if channels == 0 || channels > 4 {
        return Err(format!("jp2: {} компонентов не разложить в RGBA", channels));
    }
    if components.iter().any(|c| c.samples().len() != components[0].samples().len()) {
        return Err("jp2: субдискретизация компонентов не поддерживается".to_string());
    }
    if components[0].samples().len() != (dw as usize) * (dh as usize) {
        return Err(format!(
            "jp2: декод отдал {} сэмплов вместо {}×{}",
            components[0].samples().len(),
            dw,
            dh
        ));
    }

    let samples = decoded.data_u8();
    drop(data);

    let mut rgba = to_rgba(&samples, Pixel::named(channels), (dw as usize) * (dh as usize));
    drop(samples);

    // У файлов без собственной альфы поля гранулы приезжают нулём: у
    // Sentinel-2 ноль зарезервирован под «нет данных» (валидные значения —
    // 1..255). Прозрачность здесь, а не у потребителей: тайл — готовый RGBA.
    if matches!(channels, 1 | 3) {
        key_margins(&mut rgba, dw, dh);
    }

    let mut cascade = Cascade::new(base, dw, dh);
    cascade.push_rows(&rgba, dh, emit)?;
    cascade.finish(emit)
}

/// Прозрачность полей: заливка от краёв кадра по нулевым пикселям. Выбить
/// весь чёрный нельзя — настоящему чёрному в произвольном JP2 никто не
/// запрещал быть, — а поле гранулы от него отличается ровно тем, что
/// примыкает к краю. Внутренние нули остаются цветом.
fn key_margins(rgba: &mut [u8], w: u32, h: u32) {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return;
    }
    let zero = |rgba: &[u8], px: usize| rgba[px * 4..px * 4 + 3] == [0, 0, 0];

    let mut seen = vec![false; w * h];
    let mut queue: Vec<usize> = Vec::new();
    let push = |rgba: &[u8], seen: &mut [bool], queue: &mut Vec<usize>, px: usize| {
        if !seen[px] && zero(rgba, px) {
            seen[px] = true;
            queue.push(px);
        }
    };

    for x in 0..w {
        push(rgba, &mut seen, &mut queue, x);
        push(rgba, &mut seen, &mut queue, (h - 1) * w + x);
    }
    for y in 0..h {
        push(rgba, &mut seen, &mut queue, y * w);
        push(rgba, &mut seen, &mut queue, y * w + w - 1);
    }

    while let Some(px) = queue.pop() {
        rgba[px * 4 + 3] = 0;
        let (x, y) = (px % w, px / w);
        if x > 0 {
            push(rgba, &mut seen, &mut queue, px - 1);
        }
        if x + 1 < w {
            push(rgba, &mut seen, &mut queue, px + 1);
        }
        if y > 0 {
            push(rgba, &mut seen, &mut queue, px - w);
        }
        if y + 1 < h {
            push(rgba, &mut seen, &mut queue, px + w);
        }
    }
}

/// Пик памяти прохода при декоде в w×h: сам файл, f32-плоскости декодера
/// (до четырёх каналов), его интерливленный u8-выход и RGBA.
fn estimate(file_len: u64, w: u32, h: u32) -> u64 {
    file_len + u64::from(w) * u64::from(h) * (4 * 4 + 4 + 4)
}

/// Уровень пирамиды с такими размерами, от нулевого до `up_to`. Декод обязан
/// лежать на ceil-лестнице пирамиды — размеры мимо неё значат, что декодер
/// считает пропуск иначе, и доверять его выходу нельзя.
fn ladder_level(width: u32, height: u32, dw: u32, dh: u32, up_to: u32) -> Option<u32> {
    (0..=up_to)
        .find(|k| (pyramid::level_size(width, *k), pyramid::level_size(height, *k)) == (dw, dh))
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
/// Нужна не показу, а выбору способа чтения, и решает она его целиком.
/// Единица чтения у JPEG 2000 — **тайл-парт**, а не пакет: тело тайла читается
/// сплошняком. Значит «прочитать кусок картинки» стои́т ровно столько тайлов,
/// сколько этот кусок задевает, и у нарезанного на один тайл файла выбор куска
/// не экономит ни байта.
///
/// `resolutions` — сколько ступеней вейвлета записано. Грубее последней
/// декодер не отдаёт, и такие уровни пирамиды добираются своим делением
/// пополам.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codestream {
    /// Холст и его начало (`Xsiz`/`Ysiz`, `XOsiz`/`YOsiz`).
    pub canvas: (u32, u32),
    pub origin: (u32, u32),
    /// Шаг сетки тайлов и её начало (`XTsiz`/`YTsiz`, `XTOsiz`/`YTOsiz`).
    pub tile: (u32, u32),
    pub tile_origin: (u32, u32),
    pub components: u16,
    pub resolutions: u8,
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

/// Начало кодстрима в голове файла: тело коробки `jp2c` либо сам файл, если это
/// голый кодстрим. `None` — коробки в прочитанную голову не поместилось.
fn codestream_at(head: &[u8]) -> Option<usize> {
    if head.starts_with(CODESTREAM_MAGIC) {
        return Some(0);
    }
    let mut at = 0usize;
    while at + 8 <= head.len() {
        let len = u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as u64;
        let kind = &head[at + 4..at + 8];
        let (body, next) = match len {
            0 => (at + 8, head.len()),
            1 => {
                if at + 16 > head.len() {
                    return None;
                }
                let xlen = u64::from_be_bytes(head[at + 8..at + 16].try_into().unwrap());
                (at + 16, step(at, xlen, head.len()))
            }
            _ => (at + 8, step(at, len, head.len())),
        };
        if kind == b"jp2c" {
            return Some(body);
        }
        if next <= at {
            return None;
        }
        at = next;
    }
    None
}

/// Раскладка из головы: маркер SIZ даёт холст и сетку тайлов, первый COD —
/// число разрешений.
///
/// COD ищется обходом маркеров, а не по смещению: между SIZ и COD стоя́т
/// необязательные сегменты, и порядок их файл выбирает сам. Обход кончается на
/// SOT — дальше идут тайлы, а нам нужен главный заголовок.
pub fn codestream(head: &[u8]) -> Result<Codestream, String> {
    let at = codestream_at(head).ok_or("jp2: кодстрим не найден в голове файла")?;
    let cs = &head[at..];
    if cs.len() < 40 || cs[0] != 0xFF || cs[1] != 0x4F || cs[2] != 0xFF || cs[3] != 0x51 {
        return Err("jp2: кодстрим без SOC и SIZ".to_string());
    }
    let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let be16 = |at: usize| u16::from_be_bytes(cs[at..at + 2].try_into().unwrap());
    // SOC — два байта без длины, дальше сегмент SIZ: [маркер 2][Lsiz 2][Rsiz 2]
    // [Xsiz 4][Ysiz 4][XOsiz 4][YOsiz 4][XTsiz 4][YTsiz 4][XTOsiz 4][YTOsiz 4]
    // [Csiz 2]. Смещения считаются от маркера SIZ, а не от начала кодстрима:
    // SOC перед ним свою пару байт занимает.
    let siz = 2usize;
    let mut layout = Codestream {
        canvas: (be32(siz + 6), be32(siz + 10)),
        origin: (be32(siz + 14), be32(siz + 18)),
        tile: (be32(siz + 22), be32(siz + 26)),
        tile_origin: (be32(siz + 30), be32(siz + 34)),
        components: be16(siz + 38),
        resolutions: 0,
    };

    // Обход сегментов главного заголовка. Длина у каждого стои́т сразу за
    // маркером и считает саму себя, так что следующий маркер — через `2 + L`.
    // Обход кончается на SOT: дальше идут тайлы, а нужен главный заголовок.
    let mut walk = siz + 2 + be16(siz + 2) as usize;
    while walk + 4 <= cs.len() {
        if cs[walk] != 0xFF {
            return Err("jp2: главный заголовок кончился не маркером".to_string());
        }
        let marker = cs[walk + 1];
        if marker == 0x90 {
            break;
        }
        let length = be16(walk + 2) as usize;
        if length < 2 || walk + 2 + length > cs.len() {
            break;
        }
        // COD: [Scod 1][SGcod: порядок 1, слоёв 2, MCT 1][SPcod: ступеней 1…].
        // Ступеней вейвлета на единицу меньше, чем разрешений.
        if marker == 0x52 && length >= 12 {
            layout.resolutions = cs[walk + 9].saturating_add(1);
        }
        walk += 2 + length;
    }
    if layout.resolutions == 0 {
        return Err("jp2: кодстрим без COD в голове файла".to_string());
    }
    Ok(layout)
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

    #[test]
    fn ladder_level_locates_actual_decode_step() {
        // Нечётные стороны: ceil-деление, как у pyramid::level_size.
        assert_eq!(ladder_level(1301, 523, 1301, 523, 3), Some(0));
        assert_eq!(ladder_level(1301, 523, 651, 262, 3), Some(1));
        // Грубее запрошенного декод не бывает — глубже up_to не ищется.
        assert_eq!(ladder_level(1301, 523, 326, 131, 1), None);
        // floor-лестница (650 = 1301/2 с округлением вниз) — не наша.
        assert_eq!(ladder_level(1301, 523, 650, 262, 3), None);
    }

    #[test]
    fn budget_refuses_native_tci_but_takes_overview() {
        // TCI Sentinel-2: 10980² в родном разрешении — за потолком,
        // уровень 2 — в бюджете.
        assert!(estimate(120 << 20, 10980, 10980) > DECODE_BUDGET);
        assert!(estimate(120 << 20, 2745, 2745) <= DECODE_BUDGET);
    }

    #[test]
    fn margins_key_edge_connected_zeros_only() {
        // 3×3: нулевой угол — поле, нулевой центр — настоящий чёрный.
        let mut rgba = Vec::new();
        for v in [0u8, 5, 5, 5, 0, 5, 5, 5, 5] {
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
        key_margins(&mut rgba, 3, 3);
        let alphas: Vec<u8> = rgba.chunks(4).map(|px| px[3]).collect();
        assert_eq!(alphas, vec![0, 255, 255, 255, 255, 255, 255, 255, 255]);
        // Цвет ключёванного не трогается: усреднение взвешено по альфе, и
        // кромёжный пиксель не обязан быть чёрным заранее.
        assert_eq!(&rgba[..3], &[0, 0, 0]);
    }
}

