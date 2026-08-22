//! Съёмка и её части: как из продуктов каталога собираются снимки.
//!
//! Одну съёмку каталог держит несколькими продуктами. Иногда это одно и то же
//! в разной обработке: сырьё приёмника, снимок уровня 1 полосным TIFF, он же
//! тайловым COG. Иногда — разные измеренные величины одного пролёта: восемь
//! полос радиометра, семь газов над одной и той же полосой земли. Человеку и
//! то, и другое — один снимок, и строкой в списке он должен быть один. Какую
//! из частей показывать, решается здесь же — тем же знанием и по тем же
//! фактам.
//!
//! Всё здесь — чистые функции над фактами каталога: разбор ответа живёт в
//! catalogue.rs, правила тут, и проверяются они без сети.

use crate::proto::data_provider::{DataProduct, Part};

/// Что о продукте нужно знать, чтобы собрать снимки. Отдельно от самого
/// продукта затем, что живёт это в атрибутах каталога и наружу не едет:
/// заказчику нужен снимок, а не то, чем его собирали.
#[derive(Clone)]
pub struct Facts {
    /// Спутник словами каталога: «SENTINEL-1».
    pub platform: String,
    /// Прибор словами каталога: «TROPOMI». Нужен затем, что на одном борту их
    /// бывает несколько — см. [`acquisition`].
    pub instrument: String,
    /// Дататейк — непрерывный отрезок съёмки. Пусто у миссий, которые его не
    /// сообщают (Sentinel-2 обходится плиткой).
    pub datatake: String,
    /// Плитка сетки — «43XDB» у Sentinel-2, как её назвал каталог. Пусто у
    /// миссий без сетки и у тех, кому каталог её не сообщил.
    pub tile: String,
    /// Клетка, прочитанная из имени продукта (см. [`tile_in_name`]). Отдельно
    /// от `tile` затем, что каталогу она неизвестна: спрашивать по ней
    /// соседей — значит спрашивать про атрибут, которого у продукта нет, и
    /// получать пустой ответ. Съёмку она называет наравне с `tile`, а в
    /// запросы к каталогу не идёт.
    pub named_tile: String,
    /// Номер слайса внутри дататейка. Дататейк — это непрерывный отрезок
    /// съёмки, и режется он на слайсы; номер слайса — единственное, что
    /// называет одну и ту же съёмку одинаково у всех её частей. Пусто у миссий
    /// без нарезки и у тех частей, которым каталог его не сообщил.
    pub slice: String,
    /// Номер витка. Пусто, если каталог его не сообщил.
    pub orbit: String,
    /// Секунда начала съёмки.
    pub second: i64,
    /// Уровень обработки — см. [`level`]. `None` — каталог его не назвал.
    pub level: Option<u32>,
    /// Тип продукта словами каталога: «IW_GRDH_1S-COG».
    pub kind: String,
    pub size: u64,
    /// У продукта есть контур. Им и отличается снимок от вспомогательных
    /// данных — см. [`group`].
    pub framed: bool,
}

/// Ключ съёмки. `None` — сказать по этому продукту, что он с кем-то одна
/// съёмка, нечем.
///
/// Связывает продукты только то, что об этом сказал каталог. Одной секунды
/// мало — Sentinel-3 снимает двумя приборами разом, и два его продукта с
/// одинаковым временем это два разных снимка. Лучше оставить съёмку
/// неразобранной, чем слить в один снимок чужие.
pub fn acquisition(facts: &Facts) -> Option<String> {
    // Номер слайса — то единственное, чем части одной съёмки названы
    // одинаково. Секунда на его месте не годится: нарезка у сырья своя, и
    // начало его слайса отстоит от обработанного на четыре секунды, а у
    // комплексного — на полторы. По секунде эти части расходятся по разным
    // снимкам, и одна съёмка Sentinel-1 показывается тремя строками из пяти
    // возможных.
    if !facts.datatake.is_empty() && !facts.slice.is_empty() {
        return Some(format!(
            "по слайсу|{}|{}|{}",
            facts.platform, facts.datatake, facts.slice
        ));
    }
    // Дататейк без номера слайса и плитка сетки. Секунда нужна и там, и там: у
    // дататейка она отделяет слайсы друг от друга, у плитки — разные пролёты
    // над одной и той же клеткой.
    let tile = match facts.tile.is_empty() {
        true => &facts.named_tile,
        false => &facts.tile,
    };
    if !facts.datatake.is_empty() || !tile.is_empty() {
        return Some(format!(
            "по съёмке|{}|{}|{}|{}",
            facts.platform, facts.datatake, tile, facts.second
        ));
    }
    // Ни того, ни другого — тогда съёмку называет виток вместе с прибором и
    // секундой. Виток длится полтора часа, и один он склеил бы весь пролёт;
    // прибор нужен затем, что на борту их бывает несколько. Так лежит
    // Sentinel-5P: десять газов над одной и той же пятиминутной гранулой. Полосы
    // радиометра того же витка сюда не попадают — они начинаются минутой раньше,
    // и это отдельная съёмка, а не части этой.
    if !facts.orbit.is_empty() {
        return Some(format!(
            "по витку|{}|{}|{}|{}",
            facts.platform, facts.instrument, facts.orbit, facts.second
        ));
    }
    None
}

/// Дататейк из имени продукта Sentinel-1 — десятичным числом, как его
/// называет каталог.
///
/// Каталог называет `datatakeID` не всему архиву: у продуктов до осени 2023
/// его нет вовсе (проверено по живому каталогу — 2019 и 2022 годы пусты, с
/// октября 2023 поле заполнено). Без него правило «по слайсу» не срабатывает,
/// ключ падает на «по витку с секундой», а секунды у сырья, комплексного и
/// обработанного расходятся на две-четыре — и одна съёмка показывается тремя
/// строками из пяти возможных, ровно тем, что [`acquisition`] и предотвращает.
///
/// В имени дататейк стои́т всегда — шестнадцатеричным полем сразу за парой
/// времён съёмки и номером витка:
/// `S1A_IW_GRDH_1SDV_<начало>_<конец>_<виток>_<дататейк>_<CRC>`.
/// Ищется он от этой пары, а не по счёту полей: у `WV_SLC__1SSV` тип занимает
/// два поля с пустым между ними, а у тайловой укладки в конце добавлен `_COG`,
/// и счёт с любого края сбивается.
///
/// Читается только у имён Sentinel-1, и это не осторожность: пара времён стои́т
/// и в чужих именах, а поле за ней там значит другое. У COP-DEM
/// (`DEM1_SAR_DTE_30_<t1>_<t2>_ADS_000000_rLrO`) на месте дататейка стоят нули,
/// и все клетки одной даты слились бы в одну строку списка — проверено живым
/// каталогом: 200 продуктов из 200.
pub fn datatake_in_name(name: &str) -> Option<String> {
    let name = name.split('.').next().unwrap_or(name);
    let fields: Vec<&str> = name.split('_').collect();
    // Спутник в первом поле: `S1A`…`S1D`. Тем и опознаётся раскладка имени.
    let platform = fields.first()?;
    let sentinel1 = platform.len() == 3
        && platform.starts_with("S1")
        && platform.as_bytes()[2].is_ascii_uppercase();
    if !sentinel1 {
        return None;
    }
    let a_time = |field: &&str| {
        field.len() == 15
            && field.as_bytes()[8] == b'T'
            && field.chars().enumerate().all(|(at, sign)| at == 8 || sign.is_ascii_digit())
    };
    let pair = fields.windows(2).position(|pair| a_time(&pair[0]) && a_time(&pair[1]))?;
    // За парой времён идут виток и дататейк — оба шестизначные, но виток
    // десятичный, а дататейк шестнадцатеричный.
    let datatake = fields.get(pair + 3)?;
    let hex = |field: &&str| field.len() == 6 && field.chars().all(|sign| sign.is_ascii_hexdigit());
    hex(datatake).then(|| u64::from_str_radix(datatake, 16).ok())?.map(|value| value.to_string())
}

/// Клетка градусной сетки из имени продукта — «N03W004», как называет свои
/// плитки Sentinel-1 RTC.
///
/// Каталог о ней молчит: `tileId` у этих продуктов пуст, как пусты дататейк и
/// номер слайса, — и без плитки ключ съёмки падает на виток с секундой. А
/// виток у соседних клеток один, и секунда одна: пролёт нарезан на клетки уже
/// после съёмки, время у всех от неё. Десяток клеток по разным углам Земли
/// становился бы одной строкой списка.
///
/// Читается строго: две цифры широты, три долготы, полушария буквами и
/// подчёркивание сразу за клеткой. Совпасть с этим случайно нечему — имена
/// прочих миссий начинаются с платформы («S1A_», «S2B_»).
pub fn tile_in_name(name: &str) -> Option<String> {
    let tile = name.split('_').next()?;
    let sign = tile.as_bytes();
    let shaped = tile.len() == 7
        && matches!(sign[0], b'N' | b'S')
        && matches!(sign[3], b'E' | b'W')
        && sign[1..3].iter().all(u8::is_ascii_digit)
        && sign[4..7].iter().all(u8::is_ascii_digit);
    shaped.then(|| tile.to_string())
}

/// Суффиксы, которыми в имени продукта записана укладка, а не съёмка.
/// Климатика CDSE лежит двумя такими: `…_V3.0.1_cog` и `…_V3.0.1_nc` — одни и
/// те же величины в COG и в NetCDF.
const PACKAGING_SUFFIXES: [&str; 2] = ["_cog", "_nc"];

/// Ключ съёмки по имени — запасной, для продуктов, которых каталог не связал
/// ничем: ни дататейком, ни плиткой, ни витком.
///
/// Имена частей такой съёмки совпадают целиком, кроме хвоста, которым и названа
/// укладка. Правило узкое нарочно: полное совпадение имён ничего не сливает
/// (двух продуктов с одним именем у каталога не бывает), а сливается ровно то,
/// что отличается известным суффиксом.
///
/// Регистр при этом не важен, и стои́т суффикс не обязательно в самом конце:
/// у климатики это `…_V3.0.1_cog` без расширения вовсе, а у сторонних
/// поставщиков — `…_5415_COG.DIMA`, где после укладки идёт ещё и расширение.
/// Искать его только в хвосте и только строчным значило бы разойтись с
/// [`tiled`], который смотрит на то же самое и обоих этих случаев не
/// пропускает.
fn by_name(facts: &Facts, name: &str) -> String {
    format!("по имени|{}|{}|{}", facts.platform, stem(name), facts.second)
}

/// Имя без того, чем в нём названа укладка: без расширения контейнера и без
/// хвоста-суффикса. Снимается и то, и другое, потому что укладку записывают
/// то одним, то другим, а иногда обоими сразу: `…_5415_COG` против
/// `…_5415.DIMA` и `…_675b.DIMA` против `…_675b_COG.DIMA` — обе пары про одну
/// съёмку.
fn stem(name: &str) -> &str {
    let stem = without_extension(name);
    PACKAGING_SUFFIXES
        .iter()
        .find_map(|suffix| strip_ignoring_case(stem, suffix))
        .unwrap_or(stem)
}

/// Имя без расширения контейнера.
///
/// Расширение узнаётся по виду, а не по одной лишь точке: в именах климатики
/// точка стои́т внутри номера версии (`…_V3.0.1_nc`), и отрезанное по последней
/// точке оставило бы от имени огрызок. Настоящее расширение — короткий хвост из
/// букв и цифр, в котором есть хотя бы одна буква.
fn without_extension(name: &str) -> &str {
    let Some((head, tail)) = name.rsplit_once('.') else { return name };
    let looks_like = (2..=5).contains(&tail.len())
        && tail.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && tail.bytes().any(|byte| byte.is_ascii_alphabetic());
    match looks_like {
        true => head,
        false => name,
    }
}

/// Имя без названного хвоста, если он там есть; регистр не важен.
fn strip_ignoring_case<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    // Срезом, а не посимвольно: суффиксы здесь ASCII, а на негодной границе
    // символа срез просто не соберётся.
    let tail = name.get(cut..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &name[..cut])
}

/// Порядок предпочтения частей: меньше — раньше, первая и показывается.
///
/// Сырьё уровня 0 всегда последнее: изображения в нём нет вовсе, есть эхо
/// приёмника. Перед ним — то, чей уровень каталог не назвал вовсе: так лежат
/// служебные аннотации к съёмке (`IW_ETA__AX` приезжает неделей позже самого
/// снимка), и они мелкие, то есть по прочим правилам оказались бы впереди
/// гигабайтного снимка и стали бы показываться вместо него. Дальше уровень по
/// возрастанию — уровень 1 это сам снимок, а уровень 2 уже производные величины
/// поверх него. Тайловая укладка идёт вперёд полосной: её читают окном, а
/// полосной верхний уровень пирамиды стоил бы чтения всего файла. При прочих
/// равных — меньшая: у одной съёмки это либо та же картинка, уложенная плотнее,
/// либо величина, которую дешевле показать.
pub fn rank(facts: &Facts, name: &str) -> (u32, bool, u64) {
    let order = match facts.level {
        Some(0) => u32::MAX,
        None => u32::MAX - 1,
        Some(level) => level,
    };
    (order, !tiled(&facts.kind, name), facts.size)
}

/// Ответ на вопрос об одном продукте: он сам вместе со своими соседями по
/// съёмке — либо соседка, если его самого показать нечем. `None` — спрошенного
/// среди этих продуктов нет.
///
/// Подмена оправдана дважды и только дважды. У сырья уровня 0 изображения нет
/// вовсе: показать вместо него обработанный снимок той же съёмки — это ответ,
/// а не подлог. И та же величина, лежащая рядом тайловой укладкой, лучше
/// полосной, которую ради одного тайла пришлось бы прочитать целиком.
///
/// Во всех прочих случаях подменять нельзя, и это не придирка: части одной
/// съёмки бывают разными измеренными величинами, и на вопрос об угарном газе
/// ответить озоном значит соврать. Порядок частей при этом остаётся общий
/// (см. [`rank`]) — переезжает только пометка «показывается».
pub fn about(products: Vec<(Facts, DataProduct)>, asked: &str) -> Option<DataProduct> {
    let (kind, level, mine) = products
        .iter()
        .find(|(_, product)| product.identifier == asked)
        .map(|(facts, product)| (facts.kind.clone(), facts.level, product.clone()))?;

    let scene = group(products).into_iter().find(|scene| {
        scene.identifier == asked || scene.parts.iter().any(|part| part.identifier == asked)
    })?;

    // Тип берётся у показываемой части. У самого снимка он тот же — снимком
    // названа она и есть, — но найти её всё равно приходится: ниже по ней же
    // решается, разворачивать ли набор.
    let shown = scene.parts.iter().find(|part| part.shown).map(|part| part.product_type.clone());
    let same_quantity = shown.is_some_and(|shown| quantity(&kind) == quantity(&shown));
    if scene.identifier == asked || level == Some(0) || same_quantity {
        return Some(scene);
    }
    let parts = scene
        .parts
        .into_iter()
        .map(|part| Part { shown: part.identifier == asked, ..part })
        .collect();
    Some(DataProduct { parts, ..mine })
}

/// Величина без хвоста укладки: `IW_GRDH_1S-COG` и `IW_GRDH_1S` — снятое и
/// обработанное одинаково, а уложенное по-разному.
fn quantity(kind: &str) -> &str {
    for suffix in ["-COG", "_COG"] {
        let cut = kind.len().saturating_sub(suffix.len());
        if kind.len() > suffix.len() && kind[cut..].eq_ignore_ascii_case(suffix) {
            return &kind[..cut];
        }
    }
    kind
}

/// Часть уложена тайлово — то есть отдаёт произвольный тайл, не читая себя
/// целиком. Сказано это либо типом продукта (`IW_GRDH_1S-COG` у Sentinel-1),
/// либо хвостом имени (`…_V3.0.1_cog` у климатики): пишут одно и то же
/// по-разному, а значит оно одно и то же.
fn tiled(kind: &str, name: &str) -> bool {
    let cog = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.ends_with("-cog") || lower.ends_with("_cog")
    };
    cog(kind) || cog(name)
}

/// Уровень обработки из слова каталога. `None` — каталог о нём промолчал.
///
/// Форма у него не одна: «LEVEL1» у Sentinel-1, «S2MSI1C» у Sentinel-2. Общее
/// в них то, что уровень записан **последней** цифрой — первая у Sentinel-2
/// принадлежит имени прибора.
///
/// Промолчавший каталог — не «как у большинства», а отдельный ответ, и разница
/// не умозрительная: слова про уровень нет у служебных аннотаций к съёмке, и
/// зачтённые единицей они встают в списке частей выше самого снимка (см.
/// [`rank`]) — мелкие ведь.
pub fn level(said: &str) -> Option<u32> {
    said.bytes().rev().find(u8::is_ascii_digit).map(|digit| u32::from(digit - b'0'))
}

/// Насколько далеко от начала слайса может начинаться часть, которой каталог
/// номера слайса не назвал.
///
/// Десять секунд: части одного слайса расходятся началом на четыре секунды
/// (сырьё), а сами слайсы стоят друг от друга на двадцать пять и больше. То
/// есть окно с запасом накрывает своё и с запасом не дотягивается до соседнего.
const BESIDE_A_SLICE_S: i64 = 10;

/// Слайс дататейка: чем он назван и когда начался.
struct Slice {
    datatake: String,
    key: String,
    second: i64,
}

/// Слайсы съёмок: названные каталогом по номеру, а где он не назвал ни одного
/// — размеченные по времени.
///
/// Второе нужно затем, что номера слайса каталог сообщает не всему архиву, и
/// бывает дататейк, где его нет ни у одной части (живой пример — съёмка 191698
/// за 5 марта 2019). Прикладывать такие части не к чему, и ключ падал на
/// «дататейк с секундой», а секунды у сырья, комплексного и обработанного
/// расходятся на две-четыре — одна съёмка разъезжалась по строкам ровно тем,
/// что [`acquisition`] и предотвращает.
///
/// Окно разметки то же, что и у прикладывания: части одного слайса расходятся
/// началом на четыре секунды, а сами слайсы стоя́т друг от друга на двадцать
/// пять и больше.
///
/// Делить так позволительно только потому, что непустой дататейк бывает у
/// одной миссии — Sentinel-1: остальным каталог его не называет вовсе (у
/// Sentinel-2 на его месте `tileId`). Будь он общим у множества плиток, они
/// слились бы в одну строку.
fn slices(products: &[(Facts, DataProduct)]) -> Vec<Slice> {
    let mut named: Vec<Slice> = products
        .iter()
        .filter(|(facts, _)| !facts.slice.is_empty() && !facts.datatake.is_empty())
        .filter_map(|(facts, _)| {
            Some(Slice {
                datatake: facts.datatake.clone(),
                key: acquisition(facts)?,
                second: facts.second,
            })
        })
        .collect();

    // Части дататейков, где номера слайса нет ни у кого. По возрастанию
    // времени, чтобы разметка не зависела от порядка выдачи каталога.
    let mut nameless: Vec<&Facts> = products
        .iter()
        .map(|(facts, _)| facts)
        .filter(|facts| !facts.datatake.is_empty() && facts.slice.is_empty())
        .filter(|facts| !named.iter().any(|slice| slice.datatake == facts.datatake))
        .collect();
    nameless.sort_by(|a, b| (&a.datatake, a.second).cmp(&(&b.datatake, b.second)));

    for facts in nameless {
        let beside = named.iter().any(|slice| {
            slice.datatake == facts.datatake && (slice.second - facts.second).abs() <= BESIDE_A_SLICE_S
        });
        if !beside {
            named.push(Slice {
                datatake: facts.datatake.clone(),
                key: format!("около слайса|{}|{}|{}", facts.platform, facts.datatake, facts.second),
                second: facts.second,
            });
        }
    }
    named
}

/// Ключ слайса, рядом с которым снята эта часть. `None` — номер слайса у неё
/// свой, либо рядом такого слайса нет.
///
/// Нужно это затем, что номер слайса каталог сообщает не всем частям: у
/// Sentinel-1 его нет у `OCN`, и без этого хода производная величина уезжает в
/// собственный снимок — при том что начинается она в ту же микросекунду, что и
/// снимок, из которого её посчитали.
fn beside_a_slice(slices: &[Slice], facts: &Facts) -> Option<String> {
    if !facts.slice.is_empty() || facts.datatake.is_empty() {
        return None;
    }
    slices
        .iter()
        .filter(|slice| slice.datatake == facts.datatake)
        .min_by_key(|slice| (slice.second - facts.second).abs())
        .filter(|slice| (slice.second - facts.second).abs() <= BESIDE_A_SLICE_S)
        .map(|slice| slice.key.clone())
}

/// Продукты каталога → снимки, порядком каталога (свежие сверху).
///
/// Ключ съёмки одного продукта — то единственное, чем во всём модуле отвечают
/// на «одна ли это съёмка».
///
/// Слайсы приходят доводом, а не считаются здесь: узнаются они по всему набору
/// сразу, а ключ спрашивают по одному продукту.
fn key_of(slices: &[Slice], facts: &Facts, name: &str) -> String {
    beside_a_slice(slices, facts)
        .or_else(|| acquisition(facts))
        .unwrap_or_else(|| by_name(facts, name))
}

/// Те из набора, что сняты той же съёмкой, что и названный продукт.
///
/// Ответ здесь тот же самый, которым сводит снимки [`group`], и это не
/// вежливость. Второй ответ рядом уже был: обратный ход по ключу отбирал
/// соседей голым [`acquisition`], и у частей без номера слайса ключи
/// расходились — часть, которую поиск показывал внутри снимка, по ключу
/// оставалась одиночкой. Одна и та же строка списка при этом раскрывалась
/// по-разному в зависимости от того, откуда о ней спросили.
pub fn same_scene(
    facts: &Facts,
    name: &str,
    others: Vec<(Facts, DataProduct)>,
) -> Vec<(Facts, DataProduct)> {
    let framed: Vec<(Facts, DataProduct)> =
        others.into_iter().filter(|(facts, _)| facts.framed).collect();
    // Слайсы — по всему набору вместе с самим спрошенным: его слайс тоже
    // называет съёмку, к которой прикладываются части без номера.
    let mut all = framed.clone();
    all.push((facts.clone(), DataProduct { name: name.to_string(), ..Default::default() }));
    let slices = slices(&all);

    let key = key_of(&slices, facts, name);
    framed
        .into_iter()
        .filter(|(other, product)| key_of(&slices, other, &product.name) == key)
        .collect()
}

/// Не снимок — то, у чего нет контура. Вспомогательные данные (калибровочные
/// таблицы, эфемериды) каталог отдаёт продуктами наравне со съёмкой, и по
/// свежести они её обгоняют: срок действия таблицы записан будущей датой.
/// Очертить их на Земле нечем, показать нечем, и в списке снимков им не место.
///
/// Список частей заполняется только там, где их больше одной: одна часть —
/// это сам снимок, и повторять его собственным содержимым незачем.
pub fn group(products: Vec<(Facts, DataProduct)>) -> Vec<DataProduct> {
    let mut order: Vec<String> = Vec::new();
    let mut scenes: std::collections::HashMap<String, Vec<(Facts, DataProduct)>> =
        std::collections::HashMap::new();

    let framed: Vec<(Facts, DataProduct)> =
        products.into_iter().filter(|(facts, _)| facts.framed).collect();
    let slices = slices(&framed);

    for (facts, product) in framed {
        let key = key_of(&slices, &facts, &product.name);
        if !scenes.contains_key(&key) {
            order.push(key.clone());
        }
        scenes.entry(key).or_default().push((facts, product));
    }

    order
        .into_iter()
        .filter_map(|key| {
            let mut packed = scenes.remove(&key)?;
            packed.sort_by_key(|(facts, product)| rank(facts, &product.name));
            let parts = match packed.len() {
                0 => return None,
                1 => Vec::new(),
                _ => packed
                    .iter()
                    .enumerate()
                    .map(|(at, (facts, product))| Part {
                        identifier: product.identifier.clone(),
                        name: product.name.clone(),
                        product_type: facts.kind.clone(),
                        size: product.size,
                        folder: product.folder,
                        shown: at == 0,
                        viewable: product.viewable,
                    })
                    .collect(),
            };
            let (_, mut shown) = packed.into_iter().next()?;
            shown.parts = parts;
            Some(shown)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Факты без единой связи — с них начинается каждый набор в тестах.
    fn bare(platform: &str, kind: &str, level: u32, size: u64) -> Facts {
        Facts {
            platform: platform.to_string(),
            instrument: String::new(),
            datatake: String::new(),
            tile: String::new(),
            named_tile: String::new(),
            slice: String::new(),
            orbit: String::new(),
            second: 100,
            level: Some(level),
            kind: kind.to_string(),
            size,
            framed: true,
        }
    }

    /// Части одного слайса Sentinel-1: номер слайса у них общий, а секунда
    /// начала — своя у каждой, как их и отдаёт каталог.
    fn facts(kind: &str, level: u32, size: u64) -> Facts {
        Facts {
            datatake: "73311".to_string(),
            slice: "23".to_string(),
            second: 1_786_000_000,
            ..bare("SENTINEL-1", kind, level, size)
        }
    }

    fn product(name: &str) -> DataProduct {
        DataProduct {
            identifier: format!("eodata/{}", name),
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Каталог называет дататейк не всему архиву, а имя — всегда. Без этого
    /// одна съёмка Sentinel-1 до осени 2023 года распадалась на три строки:
    /// секунды у сырья, комплексного и обработанного расходятся.
    #[test]
    fn a_datatake_is_read_from_the_name_when_the_catalogue_keeps_quiet() {
        // Живые имена: обычное, с двойным подчёркиванием в типе и с `_COG`.
        assert_eq!(
            datatake_in_name("S1B_IW_GRDH_1SDV_20190601T001108_20190601T001137_016495_01F0AB_C0FF.SAFE"),
            Some("127147".to_string())
        );
        assert_eq!(
            datatake_in_name("S1D_WV_SLC__1SSV_20260818T000819_20260818T001004_004172_007A39_CEFE.SAFE"),
            Some("31289".to_string())
        );
        assert_eq!(
            datatake_in_name(
                "S1C_IW_GRDH_1SDV_20260818T001104_20260818T001133_009042_011F30_E912_COG.SAFE"
            ),
            Some("73520".to_string())
        );
        // Не Sentinel-1 — поля стоя́т иначе, и выдумывать дататейк нельзя.
        assert_eq!(
            datatake_in_name("S2A_MSIL2A_20260818T012111_N0512_R031_T54RWV_20260818T042717.SAFE"),
            None
        );
        assert_eq!(
            datatake_in_name(
                "S3A_SL_2_LST____20260818T000122_20260818T000422_20260818T011237_0179_143_045_0000_PS1_O_NR_005.SEN3"
            ),
            None
        );
        assert_eq!(datatake_in_name("что угодно"), None);
    }

    /// «Одна ли это съёмка» отвечают в двух местах — сведением выдачи и
    /// обратным ходом по ключу, — и ответ у них обязан быть один. У части без
    /// номера слайса (OCN) голый ключ расходится с ключом её слайса, и она
    /// оставалась одиночкой ровно тогда, когда о ней спрашивали по ключу.
    #[test]
    fn the_same_scene_is_the_same_from_both_sides() {
        let mut grd = bare("SENTINEL-1", "IW_GRDH_1S", 1, 1_600_000_000);
        grd.datatake = "73290".to_string();
        grd.slice = "17".to_string();
        grd.second = 1_786_000_000;
        let mut ocn = bare("SENTINEL-1", "IW_OCN__2S", 2, 69_000_000);
        ocn.datatake = "73290".to_string();
        ocn.second = 1_786_000_000;

        let set = vec![
            (grd.clone(), product("S1C_IW_GRDH_1SDV_A.SAFE")),
            (ocn.clone(), product("S1C_IW_OCN__2SDV_B.SAFE")),
        ];
        // Сведение выдачи кладёт их в один снимок.
        assert_eq!(group(set.clone()).len(), 1, "поиск развёл части по снимкам");
        // И обратный ход по ключу отбирает те же самые две.
        assert_eq!(
            same_scene(&ocn, "S1C_IW_OCN__2SDV_B.SAFE", set).len(),
            2,
            "по ключу часть осталась одиночкой"
        );
    }

    /// Пять упаковок одной съёмки Sentinel-1 — один снимок, и показывается
    /// тайловая: сырьё нечем показать, полосной уровень 1 читается только
    /// целиком, а уровень 2 — уже не картинка.
    #[test]
    fn one_datatake_becomes_one_scene_shown_by_its_tiled_packaging() {
        let scenes = group(vec![
            (facts("IW_RAW__0S", 0, 1_218_000_000), product("raw")),
            (facts("IW_SLC__1S", 1, 6_630_000_000), product("slc")),
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
            (facts("IW_GRDH_1S-COG", 1, 739_000_000), product("cog")),
            (facts("IW_OCN__2S", 2, 69_000_000), product("ocn")),
        ]);
        assert_eq!(scenes.len(), 1, "снимок один");
        assert_eq!(scenes[0].name, "cog");
        let order: Vec<&str> = scenes[0].parts.iter().map(|p| p.product_type.as_str()).collect();
        assert_eq!(order, ["IW_GRDH_1S-COG", "IW_GRDH_1S", "IW_SLC__1S", "IW_OCN__2S", "IW_RAW__0S"]);
        assert!(scenes[0].parts[0].shown, "показывается первая");
        assert_eq!(scenes[0].parts.iter().filter(|p| p.shown).count(), 1);
    }

    /// Тайловой упаковки нет — берётся уровень 1, а не самый маленький файл:
    /// продукт уровня 2 бывает меньше всех и картинкой не является.
    #[test]
    fn without_a_tiled_packaging_level_one_wins_over_the_smallest() {
        let scenes = group(vec![
            (facts("IW_OCN__2S", 2, 69_000_000), product("ocn")),
            (facts("IW_SLC__1S", 1, 6_630_000_000), product("slc")),
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
        ]);
        assert_eq!(scenes[0].name, "grd");
    }

    /// Съёмки одной плитки за разные пролёты — разные снимки, и пустой
    /// дататейк их не сливает.
    #[test]
    fn a_degree_tile_is_read_from_the_name_when_the_catalogue_is_silent() {
        // Так называет свои клетки Sentinel-1 RTC: ни tileId, ни дататейка, ни
        // слайса у них нет, а виток с секундой у соседних клеток общие.
        assert_eq!(tile_in_name("N03W004_2018_01_19_010910"), Some("N03W004".to_string()));
        assert_eq!(tile_in_name("S11E024_2018_01_19_010906"), Some("S11E024".to_string()));
        // Имена прочих миссий начинаются с платформы — под правило не подпадают.
        assert_eq!(tile_in_name("S1A_IW_GRDH_1SDV_20260817T070301"), None);
        assert_eq!(tile_in_name("S2B_MSIL1C_20260812T093549"), None);
        assert_eq!(tile_in_name("N03W04_2018_01_19"), None, "долгота трёхзначная");
    }

    /// Пара времён стои́т не только в именах Sentinel-1, и поле за ней у чужих
    /// значит другое. У COP-DEM там нули, и все его клетки одной даты слились
    /// бы в одну строку списка.
    #[test]
    fn a_datatake_is_read_only_from_a_sentinel_one_name() {
        assert_eq!(
            datatake_in_name("DEM1_SAR_DTE_30_20101216T101933_20140316T000041_ADS_000000_rLrO"),
            None
        );
        assert_eq!(datatake_in_name("S2B_MSIL1C_20260812T093549_N0511_R036_T33UVP_x"), None);
        // А своё имя читается по-прежнему.
        assert_eq!(
            datatake_in_name("S1B_IW_GRDH_1SDV_20190601T001108_20190601T001137_016495_01F0AB_C0FF"),
            Some("127147".to_string())
        );
    }

    /// Клетки одного витка — разные снимки: пролёт нарезан на них уже после
    /// съёмки, и время у всех от неё одно.
    #[test]
    fn degree_tiles_of_one_pass_stay_apart() {
        let rtc = |tile: &str| Facts {
            named_tile: tile.to_string(),
            orbit: "118".to_string(),
            ..bare("SENTINEL-1", "RTC", 2, 100)
        };
        assert_ne!(
            acquisition(&rtc("N03W004")).expect("ключ"),
            acquisition(&rtc("N01W006")).expect("ключ")
        );
    }

    #[test]
    fn same_tile_at_another_pass_is_another_scene() {
        let tiled = |tile: &str, second: i64| Facts {
            tile: tile.to_string(),
            second,
            ..bare("SENTINEL-2", "S2MSI1C", 1, 800)
        };
        let scenes = group(vec![
            (tiled("43XDB", 100), product("a")),
            (tiled("43XDB", 200), product("b")),
            (tiled("43WFU", 100), product("c")),
        ]);
        assert_eq!(scenes.len(), 3);
    }

    /// Один виток и одна секунда — одна съёмка: пятнадцать продуктов
    /// Sentinel-5P об одной пятиминутной грануле становятся одним снимком.
    /// Показывается самый дешёвый уровень 2 — уровень 1 у этой миссии не
    /// картинка, а спектры.
    #[test]
    fn one_orbit_and_one_second_are_one_scene() {
        let tropomi = |kind: &str, level: u32, size: u64| Facts {
            instrument: "TROPOMI".to_string(),
            orbit: "45811".to_string(),
            ..bare("SENTINEL-5P", kind, level, size)
        };
        let scenes = group(vec![
            (tropomi("L2__NO2___", 2, 63_906_515), product("no2")),
            (tropomi("L2__AER_AI", 2, 25_905_862), product("aer")),
            (tropomi("L2__SO2___", 2, 119_595_377), product("so2")),
        ]);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "aer");
        assert_eq!(scenes[0].parts.len(), 3);
    }

    /// Виток тот же, а гранула другая — снимка два: за полтора часа пролёта их
    /// два десятка, и склеивать их в один было бы враньём.
    #[test]
    fn another_granule_of_the_same_orbit_is_another_scene() {
        let granule = |second: i64| Facts {
            instrument: "TROPOMI".to_string(),
            orbit: "45811".to_string(),
            second,
            ..bare("SENTINEL-5P", "L2__NO2___", 2, 63_000_000)
        };
        let scenes = group(vec![(granule(100), product("a")), (granule(400), product("b"))]);
        assert_eq!(scenes.len(), 2);
    }

    /// Виток тот же и секунда та же, а прибор другой — снимка два: Sentinel-3
    /// снимает двумя разом, и это две разные съёмки.
    #[test]
    fn two_instruments_of_one_orbit_stay_apart() {
        let aboard = |instrument: &str, kind: &str| Facts {
            instrument: instrument.to_string(),
            orbit: "1234".to_string(),
            ..bare("SENTINEL-3", kind, 2, 40)
        };
        let scenes = group(vec![
            (aboard("SLSTR", "SL_2_WST___"), product("wst")),
            (aboard("OLCI", "OL_2_WFR___"), product("wfr")),
        ]);
        assert_eq!(scenes.len(), 2);
        assert!(scenes.iter().all(|scene| scene.parts.is_empty()), "часть одна");
    }

    /// Каталог не сказал ничего — ни дататейка, ни плитки, ни витка: снимок
    /// остаётся сам по себе. Догадываться тут не о чем.
    #[test]
    fn products_the_catalogue_did_not_link_stay_apart() {
        let scenes = group(vec![
            (bare("SENTINEL-3", "SL_2_WST___", 2, 40), product("wst")),
            (bare("SENTINEL-3", "OL_2_WFR___", 2, 40), product("wfr")),
        ]);
        assert_eq!(scenes.len(), 2);
    }

    /// Каталог их не связал, а имена расходятся одним хвостом — значит это
    /// одни и те же величины в двух укладках, и снимок у них один. Показывается
    /// COG: его читают окном, а NetCDF — только целиком.
    #[test]
    fn the_same_name_with_another_packaging_suffix_is_one_scene() {
        let clms = |size: u64| bare("PROBA-V", "land_surface_temperature", 2, size);
        let scenes = group(vec![
            (clms(27_069_429), product("c_gls_LST_202608160500_GLOBE_GEO_V3.0.1_cog")),
            (clms(23_912_978), product("c_gls_LST_202608160500_GLOBE_GEO_V3.0.1_nc")),
        ]);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].parts.len(), 2);
        assert!(scenes[0].name.ends_with("_cog"), "показывается тайловая: {}", scenes[0].name);
    }

    /// Без контура — не снимок: калибровочные таблицы каталог отдаёт наравне со
    /// съёмкой и по свежести её обгоняют.
    #[test]
    fn products_without_a_footprint_are_not_scenes() {
        let mut table = facts("GIP_R2ABCA", 1, 6_700);
        table.framed = false;
        table.datatake = String::new();
        let scenes = group(vec![
            (table, product("gipp")),
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
        ]);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "grd");
    }

    /// Спросили о сырье — отвечать надо обработанным снимком той же съёмки:
    /// изображения в сырье нет вовсе.
    #[test]
    fn a_question_about_level_zero_is_answered_by_the_processed_scene() {
        let products = vec![
            (facts("IW_RAW__0S", 0, 1_218_000_000), product("raw")),
            (facts("IW_GRDH_1S-COG", 1, 739_000_000), product("cog")),
        ];
        let answer = about(products, "eodata/raw").expect("спрошенное среди них есть");
        assert_eq!(answer.name, "cog");
        assert!(answer.parts.iter().find(|part| part.name == "cog").unwrap().shown);
    }

    /// Спросили о полосной укладке — отвечать надо тайловой: величина та же, а
    /// читается она окном, а не целиком.
    #[test]
    fn a_question_about_the_striped_packaging_is_answered_by_the_tiled_one() {
        let products = vec![
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
            (facts("IW_GRDH_1S-COG", 1, 739_000_000), product("cog")),
        ];
        assert_eq!(about(products, "eodata/grd").unwrap().name, "cog");
    }

    /// А вот другая измеренная величина — не ответ: нажали на угарный газ,
    /// показаться должен угарный газ, даже если озон рядом дешевле.
    #[test]
    fn a_question_about_one_quantity_is_not_answered_by_another() {
        let tropomi = |kind: &str, size: u64| Facts {
            instrument: "TROPOMI".to_string(),
            orbit: "45811".to_string(),
            ..bare("SENTINEL-5P", kind, 2, size)
        };
        let products = vec![
            (tropomi("L2__CO____", 33_821_960), product("co")),
            (tropomi("L2__O3__PR", 3_000_000), product("o3pr")),
        ];
        let answer = about(products, "eodata/co").expect("спрошенное среди них есть");
        assert_eq!(answer.name, "co", "спрошенное и показывается");
        // Соседи при этом никуда не делись — их порядок общий, переехала
        // только пометка.
        let shown: Vec<&str> = answer
            .parts
            .iter()
            .filter(|part| part.shown)
            .map(|part| part.name.as_str())
            .collect();
        assert_eq!(shown, ["co"]);
        assert_eq!(answer.parts.len(), 2);
    }

    /// Уровень — последняя цифра слова: первая у Sentinel-2 принадлежит имени
    /// прибора, и по ней L1C оказался бы уровнем 2.
    #[test]
    fn level_is_the_last_digit_of_the_catalogue_word() {
        assert_eq!(level("LEVEL0"), Some(0));
        assert_eq!(level("LEVEL1"), Some(1));
        assert_eq!(level("LEVEL2"), Some(2));
        assert_eq!(level("S2MSI1C"), Some(1));
        assert_eq!(level("S2MSI2A"), Some(2));
        assert_eq!(level(""), None, "каталог промолчал — это отдельный ответ");
    }

    /// Части одного слайса сводятся номером слайса, а не секундой: у сырья
    /// нарезка своя, и начинается его слайс на четыре секунды раньше
    /// обработанного. По секунде одна съёмка разошлась бы на три строки.
    #[test]
    fn one_slice_is_one_scene_however_its_parts_start() {
        let part = |kind: &str, level: u32, size: u64, second: i64| Facts {
            second: 1_786_000_000 + second,
            ..facts(kind, level, size)
        };
        let scenes = group(vec![
            (part("IW_RAW__0S", 0, 1_218_000_000, -4), product("raw")),
            (part("IW_SLC__1S", 1, 6_630_000_000, -1), product("slc")),
            (part("IW_GRDH_1S", 1, 1_641_000_000, 0), product("grd")),
            (part("IW_GRDH_1S-COG", 1, 739_000_000, 0), product("cog")),
        ]);
        assert_eq!(scenes.len(), 1, "слайс один");
        assert_eq!(scenes[0].parts.len(), 4);
        assert_eq!(scenes[0].name, "cog");

        // А соседний слайс того же дататейка — другая съёмка: их разделяет
        // двадцать пять секунд, и окно до них не дотягивается.
        let next = group(vec![
            (part("IW_GRDH_1S", 1, 1_641_000_000, 0), product("here")),
            (Facts { slice: "24".to_string(), ..part("IW_GRDH_1S", 1, 1_641_000_000, 25) },
                product("next")),
        ]);
        assert_eq!(next.len(), 2);
    }

    /// Часть без номера слайса прикладывается к тому слайсу, рядом с которым
    /// снята: `OCN` каталог номером не снабжает вовсе, а начинается он в ту же
    /// секунду, что и снимок, из которого посчитан.
    #[test]
    fn a_part_without_a_slice_number_joins_the_one_beside_it() {
        let ocn = Facts { slice: String::new(), ..facts("IW_OCN__2S", 2, 69_000_000) };
        let scenes = group(vec![
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
            (ocn, product("ocn")),
        ]);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].parts.len(), 2);

        // Далеко от всякого слайса — сам по себе: приложить его не к чему.
        let far = Facts {
            slice: String::new(),
            second: 1_786_000_600,
            ..facts("IW_OCN__2S", 2, 69_000_000)
        };
        let apart = group(vec![
            (facts("IW_GRDH_1S", 1, 1_641_000_000), product("grd")),
            (far, product("ocn")),
        ]);
        assert_eq!(apart.len(), 2);
    }

    /// Дататейк, которому каталог не назвал ни одного номера слайса, всё равно
    /// держит свои части вместе — их связывает он сам.
    ///
    /// Такие в каталоге есть: у съёмки 191698 (5 марта 2019) номера слайса нет
    /// ни у комплексного, ни у производной величины, и начинаются они в одну
    /// секунду. Приложить их не к чему — слайсов, рядом с которыми они сняты,
    /// не существует, — так что держит их ключ «по съёмке».
    #[test]
    fn a_datatake_without_any_slice_numbers_still_holds_its_parts() {
        let slc = Facts {
            datatake: "191698".to_string(),
            slice: String::new(),
            ..facts("IW_SLC__1S", 1, 7_800_000_000)
        };
        let ocn = Facts {
            datatake: "191698".to_string(),
            slice: String::new(),
            ..facts("IW_OCN__2S", 2, 69_000_000)
        };
        let scenes = group(vec![(slc.clone(), product("slc")), (ocn, product("ocn"))]);
        assert_eq!(scenes.len(), 1, "одна секунда на двоих — один ключ");
        assert_eq!(scenes[0].parts.len(), 2);

        // И держится это не совпадением секунд: у сырья, комплексного и
        // обработанного они расходятся на две-четыре, и по секунде такая
        // съёмка разъехалась бы по строкам. Дататейк без номеров слайса
        // размечается по времени тем же окном, что и прикладывание.
        let nameless = |kind: &str, level: u32, second: i64| Facts {
            datatake: "191698".to_string(),
            slice: String::new(),
            second,
            ..facts(kind, level, 1_500_000_000)
        };
        let together = group(vec![
            (nameless("IW_SLC__1S", 1, 1_786_000_000), product("slc")),
            (nameless("IW_RAW__0S", 0, 1_786_000_004), product("raw")),
        ]);
        assert_eq!(together.len(), 1, "четыре секунды — это одна съёмка");
        assert_eq!(together[0].parts.len(), 2);

        // А соседний слайс того же дататейка — уже другая съёмка: слайсы стоя́т
        // друг от друга на двадцать пять секунд и больше.
        let apart = group(vec![
            (nameless("IW_SLC__1S", 1, 1_786_000_000), product("slc")),
            (nameless("IW_SLC__1S", 1, 1_786_000_030), product("next")),
        ]);
        assert_eq!(apart.len(), 2, "разные слайсы одного дататейка — разные съёмки");
    }

    /// Уровень, не названный каталогом, не делает продукт снимком. Служебная
    /// аннотация к съёмке приезжает неделей позже, весит впятеро меньше — и по
    /// прежнему правилу «не сказано, значит уровень 1» вставала бы впереди
    /// самого снимка, а без тайловой упаковки и показывалась бы вместо него.
    #[test]
    fn an_unnamed_level_never_outranks_the_image() {
        let annotation = Facts { level: None, ..facts("IW_ETA__AX", 1, 59_345_163) };
        let scenes = group(vec![
            (annotation, product("eta")),
            (facts("IW_GRDH_1S", 1, 1_986_444_206), product("grd")),
        ]);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "grd", "показывается снимок, а не аннотация к нему");
        assert!(scenes[0].parts[0].shown);
        assert_eq!(scenes[0].parts[1].product_type, "IW_ETA__AX");
    }

    /// Хвост укладки узнаётся независимо от регистра и от того, стои́т ли он в
    /// самом конце имени или перед расширением. Иначе одна и та же съёмка,
    /// уложенная двумя способами, показывалась бы двумя строками.
    #[test]
    fn packaging_is_stripped_wherever_it_stands() {
        assert_eq!(stem("c_gls_LST_202608160500_GLOBE_GEO_V3.0.1_cog"), "c_gls_LST_202608160500_GLOBE_GEO_V3.0.1");
        assert_eq!(stem("c_gls_LST_202608160500_GLOBE_GEO_V3.0.1_nc"), "c_gls_LST_202608160500_GLOBE_GEO_V3.0.1");
        // Верхний регистр и расширение после укладки.
        assert_eq!(stem("EW02_WV1_MS4_OR_TOU_1234_675b_COG.DIMA"), "EW02_WV1_MS4_OR_TOU_1234_675b");
        assert_eq!(stem("EW02_WV1_MS4_OR_TOU_1234_675b.DIMA"), "EW02_WV1_MS4_OR_TOU_1234_675b");
        // Укладка в конце против расширения у близнеца — тоже одна съёмка.
        assert_eq!(stem("EW02_WV1_MS4_OR_TOU_1234_5415_COG"), "EW02_WV1_MS4_OR_TOU_1234_5415");
        assert_eq!(stem("EW02_WV1_MS4_OR_TOU_1234_5415.DIMA"), "EW02_WV1_MS4_OR_TOU_1234_5415");
        // Версия с точками расширением не считается: иначе от имени остался бы огрызок.
        assert_eq!(without_extension("thing_V3.0.1"), "thing_V3.0.1");
    }
}
