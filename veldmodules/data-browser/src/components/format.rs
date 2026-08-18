//! components/format.rs — числа и время словами.
//!
//! Одно место на всё приложение: размер в строке списка, в подзаголовке и в
//! статусной строке — это один и тот же размер, и написан он должен быть
//! одинаково.

/// Единицы по возрастанию; шаг — 1024.
const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];

/// «58,7 МБ», «42 КБ», «4,2 ГБ». Десятичная запятая — как в остальном
/// интерфейсе; дробная часть только там, где она что-то различает: у больших
/// чисел она шум, у байтов её нет вовсе.
pub fn bytes(value: u64) -> String {
    let (scaled, unit) = scale(value);
    if unit == 0 || scaled >= 10.0 {
        format!("{:.0} {}", scaled, UNITS[unit])
    } else {
        format!("{:.1} {}", scaled, UNITS[unit]).replace('.', ",")
    }
}

/// «50,9/58,7 МБ» — сделано из всего. Единица одна на оба числа: они об одном
/// и том же файле, и разные единицы рядом читаются как разные величины.
pub fn progress(done: u64, total: u64) -> String {
    let (_, unit) = scale(total);
    let show = |value: u64| {
        let scaled = value as f64 / 1024f64.powi(unit as i32);
        if unit == 0 || scaled >= 10.0 {
            format!("{:.0}", scaled)
        } else {
            format!("{:.1}", scaled).replace('.', ",")
        }
    };
    format!("{}/{} {}", show(done), show(total), UNITS[unit])
}

fn scale(value: u64) -> (f64, usize) {
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    (scaled, unit)
}

const MONTHS: [&str; 12] = [
    "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
];
const DAY: i64 = 86_400;

/// «сегодня, 10:41», «вчера, 14:08», «4 авг, 09:12», «1 мая 1991».
///
/// Чем дальше событие, тем грубее подпись: время сегодняшнего файла различает
/// его среди соседей, время файла из 1991 года — нет.
///
/// Время всемирное, не местное: часового пояса у `wasm32-wasip1` нет — ни
/// смещения, ни базы правил, и взять их модулю неоткуда.
pub fn date(when: i64, now: i64) -> String {
    if when <= 0 {
        return String::new();
    }
    let (year, month, day) = veldsdk::time::civil_from_unix(when);
    let clock = format!("{:02}:{:02}", when.div_euclid(3600).rem_euclid(24), when.div_euclid(60).rem_euclid(60));

    let today = now.div_euclid(DAY);
    match today - when.div_euclid(DAY) {
        0 => format!("сегодня, {}", clock),
        1 => format!("вчера, {}", clock),
        _ if year == veldsdk::time::civil_from_unix(now).0 => format!("{} {}, {}", day, MONTHS[month as usize - 1], clock),
        _ => format!("{} {} {}", day, MONTHS[month as usize - 1], year),
    }
}

/// Дата из набранного руками — обратная сторона [`date`]: `2026-08-13`, а
/// заодно `13.08.2026`, потому что вводят и так. Возвращает полночь этого дня
/// в unix-секундах; `None` — набрано ещё не число.
///
/// Разбор здесь, рядом с записью: это одна пара, и разойтись им негде.
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(['-', '.', '/']).collect();
    let [first, month, last] = parts[..] else { return None };
    let (year, day) = match first.len() {
        4 => (first, last),
        _ => (last, first),
    };
    let year: i64 = year.parse().ok()?;
    let month: i64 = month.parse().ok()?;
    let day: i64 = day.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1970..=9999).contains(&year) {
        return None;
    }
    Some(veldsdk::time::days_from_civil(year, month, day) * DAY)
}

/// Обрезает середину: у имён снимков различаются и начало (миссия, дата), и
/// хвост (полоса, расширение), а совпадает как раз то, что посередине.
pub fn ellipsize(text: &str, limit: usize) -> String {
    let length = text.chars().count();
    if length <= limit || limit < 4 {
        return text.to_string();
    }
    let head = limit.div_ceil(2) - 1;
    let tail = limit - head - 1;
    let chars: Vec<char> = text.chars().collect();
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[length - tail..].iter().collect::<String>(),
    )
}

/// Сейчас — в unix-секундах. Нужно только для подписи времени: «сегодня» и
/// «вчера» без точки отсчёта не существуют.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// «1 файл», «2 файла», «5 файлов» — иначе счётчик читается как заготовка.
/// Формы идут в порядке «один», «два», «пять».
pub fn plural(count: usize, forms: [&'static str; 3]) -> &'static str {
    let tail = count % 100;
    match (tail, tail % 10) {
        (11..=14, _) => forms[2],
        (_, 1) => forms[0],
        (_, 2..=4) => forms[1],
        _ => forms[2],
    }
}

/// «3 снимка, 2 файла» — счётчики подзаголовка через запятую.
///
/// Одно место на все списки: каждый считает своё (снимки и папки, слои и
/// контуры), но подписаны эти числа одинаково — иначе соседние вкладки говорят
/// об одном и том же разными словами.
///
/// Пустых слов в подписи нет: чего нет, о том и не сказано, — поэтому нулевые
/// счётчики молчат, а из одних нулей выходит пустая строка. Назвать её словом
/// умеет только сам вид: у пустой папки и у пустого «Скачанного» причины
/// молчать разные.
pub fn counted(parts: &[(usize, [&'static str; 3])]) -> String {
    parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, forms)| format!("{} {}", count, plural(*count, *forms)))
        .collect::<Vec<String>>()
        .join(", ")
}

/// Сколько знаков моноширинного текста помещается в отведённую ширину.
/// Ширина знака выведена из размера: у моноширинного шрифта она одна на все
/// знаки, и это единственный текст, который можно померить, не спрашивая
/// рендерер.
pub fn mono_fit(width: f32, size: f32) -> usize {
    const ADVANCE: f32 = 0.6;
    (width / (size * ADVANCE)).max(0.0) as usize
}

/// Сколько места займёт подпись обычным шрифтом — оценкой сверху.
///
/// Точную ширину знает только рендерер: шрифт и его метрики живут в ui-service,
/// а модуль собирает разметку, ничего не измеряя. Оценки хватает там, где
/// ответ нужен пороговый — влезает полоса рычагов в строку или нет
/// (см. `controls::bar`).
///
/// Оценка именно сверху: кириллица основного шрифта занимает около 0,46 кегля
/// на знак, латиница с цифрами — меньше, и половина кегля перекрывает обе.
/// Ошибка в большую сторону разворачивает полосу на строку чуть раньше, чем
/// стало тесно; ошибка в меньшую оставляет подпись обрезанной — ровно то, ради
/// чего ширина и считается.
pub fn text_width(text: &str, size: f32) -> f32 {
    const ADVANCE: f32 = 0.5;
    text.chars().count() as f32 * size * ADVANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пара с `date`: то, что она пишет днём, разбор возвращает обратно днём.
    #[test]
    fn дата_разбирается_обоими_привычными_способами() {
        let day = parse_date("2026-08-13").expect("ISO");
        assert_eq!(parse_date("13.08.2026"), Some(day));
        assert_eq!(veldsdk::time::civil_from_unix(day), (2026, 8, 13));
    }

    /// Набранное наполовину — ещё не дата, и молча считать её нулём нельзя:
    /// ноль в запросе значит «край не задан», а не «начало эпохи».
    #[test]
    fn недобранное_датой_не_становится() {
        for text in ["", "2026-08-", "завтра", "2026-13-01", "2026-08-32", "1900-01-01"] {
            assert_eq!(parse_date(text), None, "{}", text);
        }
    }

    /// Форма выбирается по последним двум разрядам, а не по последнему: у
    /// одиннадцати и двадцати одного он один и тот же, а слова разные.
    #[test]
    fn счётчик_склоняет_по_последним_двум_разрядам() {
        let files = |count| counted(&[(count, ["файл", "файла", "файлов"])]);
        assert_eq!(files(1), "1 файл");
        assert_eq!(files(2), "2 файла");
        assert_eq!(files(5), "5 файлов");
        assert_eq!(files(11), "11 файлов");
        assert_eq!(files(21), "21 файл");
        assert_eq!(files(111), "111 файлов");
    }

    /// Чего нет, о том и не сказано: нулевой счётчик молчит, а из одних нулей
    /// выходит пустая строка — назвать её словом умеет только сам вид.
    #[test]
    fn нулевые_счётчики_в_подпись_не_идут() {
        let parts = [
            (3usize, ["снимок", "снимка", "снимков"]),
            (0, ["папка", "папки", "папок"]),
            (2, ["файл", "файла", "файлов"]),
        ];
        assert_eq!(counted(&parts), "3 снимка, 2 файла");
        assert_eq!(counted(&[(0usize, ["слой", "слоя", "слоёв"])]), "");
        assert_eq!(counted(&[]), "");
    }
}
