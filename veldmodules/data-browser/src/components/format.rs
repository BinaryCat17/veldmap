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

/// «51/59 МБ», «4,2/9,0 ГБ» — сделано из всего. Единица одна на оба числа: они
/// об одном и том же файле, и разные единицы рядом читаются как разные
/// величины. Дробная часть — по тому же правилу, что у [`bytes`]: у чисел
/// крупнее десяти она шум, и в бегущем счётчике закачки — шум мелькающий.
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

/// Обрезает середину: имя, у которого не видно ни начала, ни хвоста, не
/// опознать вовсе, а середина — обычно то, что у соседей и так совпадает.
///
/// Обрезает вслепую: что именно совпадает у соседей, знает список, а не
/// строка (см. [`shared`]).
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

/// Обрезает хвост: фраза о беде читается с начала — суть у неё впереди, а
/// хвост договаривает, — и середина, вырезанная как у имени, оставляла бы от
/// приговора два обрывка. Как и [`ellipsize`], ниже четырёх знаков не режет.
pub fn head(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit || limit < 4 {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(limit - 1).collect();
    kept.truncate(kept.trim_end().len());
    format!("{}…", kept)
}

/// Имя величины файла многих величин — последний сегмент её пути в файле:
/// `/PRODUCT/carbonmonoxide_total_column` → `carbonmonoxide_total_column`.
/// Группы до него — устройство файла, а не имя.
pub fn variable_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// Величина словами: имя, единицы и как назвал её файл — что из этого файл
/// назвал: «carbonmonoxide_total_column, mol m-2 — Vertically integrated CO
/// column density». Имя первым: оно короче и по нему величину узнают; единицы
/// сразу за ним, потому что слова файла бывают в полторы строки и режутся с
/// хвоста ([`head`]) — единицы за ними пропадали бы первыми.
pub fn variable(path: &str, said: &str, units: &str) -> String {
    let mut text = variable_name(path).to_string();
    if !units.is_empty() {
        text = format!("{}, {}", text, units);
    }
    if !said.is_empty() {
        text = format!("{} — {}", text, said);
    }
    text
}

/// Делит `room` знаков между частями длиной `lengths`: короткие берут своё
/// целиком, длинные делят остаток поровну. Короткие — первыми: часть, которой
/// хватает её доли, отдаёт неиспользованное длинным, и ни одна не режется,
/// пока сумма влезает. Так собирается подсказка из имени файла, величины и
/// беды: одна строка на троих, и каждый в ней узнаваем.
pub fn share(lengths: &[usize], room: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&at| lengths[at]);
    let mut shares = vec![0; lengths.len()];
    let mut left = room;
    for (rank, &at) in order.iter().enumerate() {
        shares[at] = lengths[at].min(left / (lengths.len() - rank));
        left -= shares[at];
    }
    shares
}

/// Что у всех имён страницы одинаково: сколько знаков совпало в начале и
/// сколько в хвосте.
///
/// Совпавшее ничего не различает, а место занимает — и занимает его как раз
/// там, где смотрят. У имён Copernicus совпадают именно края: миссия с типом
/// продукта в начале, базовая линия обработки с расширением в конце, а
/// различает середина — время съёмки, виток, плитка. Обрезанная посередине,
/// страница каталога превращается в два десятка строк «S3A_SL_2_A…R_003.SEN3»,
/// в которых не выбрать ни одной.
///
/// Считается по показанному, а не по всему списку: сличают глазами страницу.
/// Ответ один на неё всю — иначе соседние строки резались бы в разных местах и
/// сравнивать пришлось бы разное.
///
/// Пусто, когда резать незачем (самое длинное имя и так влезает) или нечего
/// (имена разные с первого знака). Совпавшее не съедает имя целиком: у
/// одинаковых строк резать нечего, и режется тогда ничего.
pub fn shared(names: &[&str], limit: usize) -> (usize, usize) {
    let Some(longest) = names.iter().map(|name| name.chars().count()).max() else {
        return (0, 0);
    };
    if names.len() < 2 || longest <= limit {
        return (0, 0);
    }
    let shortest = names.iter().map(|name| name.chars().count()).min().unwrap_or(0);
    let chars: Vec<Vec<char>> = names.iter().map(|name| name.chars().collect()).collect();
    let same = |at: &dyn Fn(&Vec<char>) -> Option<char>| -> bool {
        let first = at(&chars[0]);
        first.is_some() && chars.iter().all(|name| at(name) == first)
    };

    let mut head = 0;
    while head < shortest && same(&|name: &Vec<char>| name.get(head).copied()) {
        head += 1;
    }
    let mut tail = 0;
    // Хвост не залезает в голову: у имён, совпавших целиком, они сошлись бы
    // посередине и вычли бы одно и то же дважды.
    while head + tail < shortest && same(&|name: &Vec<char>| name.get(name.len() - 1 - tail).copied())
    {
        tail += 1;
    }
    // От самого короткого имени обязан остаться хоть знак: строка из одних
    // многоточий не имя.
    while head + tail >= shortest && tail > 0 {
        tail -= 1;
    }
    if head + tail >= shortest {
        return (0, 0);
    }
    // Срезанное помечается многоточием, и знак под него берётся из того же
    // места. Значит, край короче двух знаков срезать незачем: он не освободит
    // ничего, а имя станет читаться на знак хуже. Так и выходит на странице
    // разных миссий, где совпадает одна буква «S».
    (if head > 1 { head } else { 0 }, if tail > 1 { tail } else { 0 })
}

/// Имя строки списка: в отведённое число знаков, и режется в нём первым то,
/// что у соседей одинаково (см. [`shared`]).
///
/// Общее срезается не всё, а сколько нужно: место, освободившееся сверх
/// различающегося, возвращается началу имени — по нему его и узнают. У
/// страницы плиток Sentinel-2 различаются три знака кода плитки, и обрезанная
/// до них строка различима, но неопознаваема; с возвратом начала выходит
/// «S2A_MSIL2A_20260…RWV…» — и то, и другое сразу.
///
/// Срезанное с краёв помечено многоточием так же, как срезанное в середине:
/// иначе укороченное имя не отличить от полного.
pub fn distinct(text: &str, limit: usize, shared: (usize, usize)) -> String {
    let (head, tail) = shared;
    let chars: Vec<char> = text.chars().collect();
    if (head == 0 && tail == 0) || head + tail >= chars.len() || limit < 4 {
        return ellipsize(text, limit);
    }
    let body: Vec<char> = chars[head..chars.len() - tail].to_vec();
    let trail = usize::from(tail > 0);

    // Начало оставляем настолько, насколько различающееся оставляет место; на
    // ведущее многоточие при этом тоже нужен знак.
    let room = limit.saturating_sub(trail);
    let keep = room.saturating_sub(body.len() + 1).min(head);
    let lead = usize::from(keep < head);
    let body: String = body.into_iter().collect();

    format!(
        "{}{}{}{}",
        chars[..keep].iter().collect::<String>(),
        if lead > 0 { "…" } else { "" },
        ellipsize(&body, limit.saturating_sub(keep + lead + trail)),
        if trail > 0 { "…" } else { "" },
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

    /// Хвост режется, начало остаётся: суть фразы о беде впереди. Пробел
    /// перед многоточием не остаётся, короткое и совсем узкий предел — как есть.
    #[test]
    fn a_head_cut_keeps_the_start_of_the_phrase() {
        assert_eq!(head("не подробнее превью: подробный растр", 20), "не подробнее превью…");
        assert_eq!(head("не подробнее превью", 40), "не подробнее превью");
        assert_eq!(head("не подробнее", 3), "не подробнее");
        assert_eq!(head("аб вг", 4), "аб…", "пробел перед многоточием висит");
    }

    /// Имя величины — последний сегмент пути; единицы и слова дописываются
    /// только названные, единицы раньше слов.
    #[test]
    fn a_variable_is_named_by_its_last_segment_then_its_words() {
        assert_eq!(variable_name("/PRODUCT/carbonmonoxide_total_column"), "carbonmonoxide_total_column");
        assert_eq!(variable_name("F1_BT_fn"), "F1_BT_fn");
        assert_eq!(
            variable("/PRODUCT/carbonmonoxide_total_column", "Vertically integrated CO column", "mol m-2"),
            "carbonmonoxide_total_column, mol m-2 — Vertically integrated CO column"
        );
        assert_eq!(variable("/F1_BT_fn", "", "K"), "F1_BT_fn, K");
        assert_eq!(variable("/x", "brightness", ""), "x — brightness");
        assert_eq!(variable("/x", "", ""), "x");
    }

    /// Короткие части берут своё, длинные делят остаток поровну; влезающее не
    /// режется.
    #[test]
    fn a_line_is_shared_shortest_first() {
        assert_eq!(share(&[11, 89, 55], 84), vec![11, 37, 36]);
        assert_eq!(share(&[10, 20], 100), vec![10, 20]);
        assert_eq!(share(&[50, 50], 60), vec![30, 30]);
        assert_eq!(share(&[], 60), Vec::<usize>::new());
    }

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

    /// Страница каталога: имена расходятся только серединой, и режется у них
    /// как раз то, что совпало. Иначе все двадцать строк выглядят одинаково.
    #[test]
    fn страница_режется_по_тому_чем_строки_отличаются() {
        let names = [
            "S3A_SL_2_AOD____20260818T201914_20260818T202213_0180_143_057______MAR_O_NR_003.SEN3",
            "S3A_SL_2_AOD____20260818T201614_20260818T201912_0179_143_057______MAR_O_NR_003.SEN3",
            "S3A_SL_2_AOD____20260818T201314_20260818T201612_0179_143_057______MAR_O_NR_003.SEN3",
        ];
        let shared = shared(&names, 21);
        assert!(shared.0 > 0 && shared.1 > 0, "края не совпали: {:?}", shared);
        let shown: Vec<String> = names.iter().map(|name| distinct(name, 21, shared)).collect();
        assert_eq!(shown.len(), 3);
        assert_ne!(shown[0], shown[1], "строки неразличимы: {}", shown[0]);
        assert_ne!(shown[1], shown[2], "строки неразличимы: {}", shown[1]);
        for one in &shown {
            assert!(one.chars().count() <= 21, "{} знаков: {}", one.chars().count(), one);
            assert!(one.starts_with('…') && one.ends_with('…'), "срез не помечен: {}", one);
        }

        // Прежнее правило на этих же именах даёт три одинаковые строки — ради
        // этого сравнение здесь и стоит.
        let blind: Vec<String> = names.iter().map(|name| ellipsize(name, 21)).collect();
        assert_eq!(blind[0], blind[1]);
    }

    /// Резать незачем, пока имя влезает целиком: обрезанное без нужды теряет
    /// то, по чему строку узнают в другом списке.
    #[test]
    fn помещающееся_имя_не_режется() {
        let names = ["одна_и_та_же_шапка_A", "одна_и_та_же_шапка_B"];
        assert_eq!(shared(&names, 40), (0, 0));
        assert_eq!(distinct(names[0], 40, (0, 0)), names[0]);
    }

    /// Имена, совпавшие целиком (или одно на всю страницу), резать нечем — и
    /// правило отступает к слепому многоточию, а не к строке из одних точек.
    #[test]
    fn совпавшему_целиком_резать_нечего() {
        let same = ["S1C_IW_GRDH_1SDV_20260818T000000.SAFE"; 3];
        assert_eq!(shared(&same, 21), (0, 0));
        assert_eq!(shared(&[same[0]], 21), (0, 0));
        assert_eq!(shared(&[], 21), (0, 0));
        // Короткое имя рядом с длинными: хвост не залезает в голову.
        let mixed = ["AxB", "AyB", "AzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzB"];
        let (head, tail) = shared(&mixed, 8);
        assert!(head + tail < 3, "съедено всё короткое имя: {:?}", (head, tail));
    }

    /// Разные с первого знака — резать по краям нечего, работает прежнее
    /// правило. Совпавшая одна буква — тоже: место под многоточие она не
    /// окупает.
    #[test]
    fn разное_с_первого_знака_режется_как_прежде() {
        let names = [
            "S1C_IW_GRDH_1SDV_20260818T000000_009054_011F94_A43C.SAFE",
            "LC08_L1TP_170025_20260818_20260818_02_T1",
        ];
        assert_eq!(shared(&names, 21), (0, 0));

        let missions = [
            "S3B_SR_1_SRA_A__20260818T110337_20260818T111337_0600_123_293.SEN3",
            "S2B_MSIL1C_20260818T093549_N0511_R036_T35UNV_20260818T113217.SAFE",
        ];
        assert_eq!(shared(&missions, 21), (0, 0));
    }

    /// Различающегося на странице бывает три знака — код плитки Sentinel-2, —
    /// и обрезанное до них имя различимо, но не опознаваемо. Освободившееся
    /// место возвращается началу: видно и что это, и какая плитка.
    #[test]
    fn освободившееся_место_возвращается_началу_имени() {
        let names = [
            "S2A_MSIL2A_20260818T012111_N0512_R031_T54RWV_20260818T042717.SAFE",
            "S2A_MSIL2A_20260818T012111_N0512_R031_T54RXV_20260818T042717.SAFE",
            "S2A_MSIL2A_20260818T012111_N0512_R031_T54SUA_20260818T042717.SAFE",
        ];
        let shared = shared(&names, 21);
        let shown: Vec<String> = names.iter().map(|name| distinct(name, 21, shared)).collect();
        for (name, one) in names.iter().zip(&shown) {
            assert!(one.chars().count() <= 21, "{} знаков: {}", one.chars().count(), one);
            assert!(one.starts_with("S2A_MSIL2A"), "имя неопознаваемо: {}", one);
            let tile = &name[41..44];
            assert!(one.contains(tile), "нет различающего куска {}: {}", tile, one);
        }
        assert_ne!(shown[0], shown[1]);
        assert_ne!(shown[1], shown[2]);
    }


    /// Размер пишется одинаково везде, где его пишут, и дробная часть стои́т
    /// ровно там, где она что-то различает.
    #[test]
    fn размер_пишется_одной_меркой() {
        assert_eq!(bytes(0), "0 Б", "у байтов дробной части нет вовсе");
        assert_eq!(bytes(512), "512 Б");
        assert_eq!(bytes(1536), "1,5 КБ", "десятичная запятая, как в остальном интерфейсе");
        assert_eq!(bytes(12 * 1024), "12 КБ", "у большого числа дробь — шум");
        assert_eq!(bytes(1024 * 1024 * 1024), "1,0 ГБ");
    }

    /// «Сделано из всего» — одной единицей на оба числа: они об одном и том же
    /// файле, и разные единицы рядом читаются как разные величины. Из этой
    /// пары собрана всякая подпись хода, и мерена по ней колонка ЗАГРУЗКА.
    #[test]
    fn ход_пишется_одной_единицей_на_оба_числа() {
        let said = progress(50 * 1024 * 1024 + 943718, 58 * 1024 * 1024 + 734003);
        assert_eq!(said, "51/59 МБ", "у чисел крупнее десяти дробь — мелькающий шум");
        assert_eq!(said.matches("МБ").count(), 1, "единица названа дважды");

        // А у мелких она различает: 4,2 ГБ и 4,9 ГБ — это два разных файла.
        assert_eq!(progress(4508876800, 9663676416), "4,2/9,0 ГБ");

        // Единицу задаёт знаменатель: иначе мелкое начало большой закачки
        // писалось бы килобайтами против гигабайт.
        assert_eq!(progress(1024, 4 * 1024 * 1024 * 1024), "0,0/4,0 ГБ");

        // Пустой знаменатель делить не на что — и не делим.
        assert_eq!(progress(0, 0), "0/0 Б");
    }
}
