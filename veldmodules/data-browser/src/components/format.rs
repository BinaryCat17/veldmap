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
    let (year, month, day) = civil_from_unix(when);
    let clock = format!("{:02}:{:02}", when.div_euclid(3600).rem_euclid(24), when.div_euclid(60).rem_euclid(60));

    let today = now.div_euclid(DAY);
    match today - when.div_euclid(DAY) {
        0 => format!("сегодня, {}", clock),
        1 => format!("вчера, {}", clock),
        _ if year == civil_from_unix(now).0 => format!("{} {}, {}", day, MONTHS[month as usize - 1], clock),
        _ => format!("{} {} {}", day, MONTHS[month as usize - 1], year),
    }
}

/// Unix-секунды → год, месяц, день. Алгоритм Хиннанта: сдвиг эры на 1 марта
/// делает год без високосного дня непрерывным, и все три числа считаются без
/// таблиц и без цикла по годам.
fn civil_from_unix(seconds: i64) -> (i64, u32, u32) {
    let days = seconds.div_euclid(DAY) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
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

/// Сколько знаков моноширинного текста помещается в отведённую ширину.
/// Ширина знака выведена из размера: у моноширинного шрифта она одна на все
/// знаки, и это единственный текст, который можно померить, не спрашивая
/// рендерер.
pub fn mono_fit(width: f32, size: f32) -> usize {
    const ADVANCE: f32 = 0.6;
    (width / (size * ADVANCE)).max(0.0) as usize
}
