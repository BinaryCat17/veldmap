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
pub struct Facts {
    /// Спутник словами каталога: «SENTINEL-1».
    pub platform: String,
    /// Прибор словами каталога: «TROPOMI». Нужен затем, что на одном борту их
    /// бывает несколько — см. [`acquisition`].
    pub instrument: String,
    /// Дататейк — непрерывный отрезок съёмки. Пусто у миссий, которые его не
    /// сообщают (Sentinel-2 обходится плиткой).
    pub datatake: String,
    /// Плитка сетки — «43XDB» у Sentinel-2. Пусто у миссий без сетки.
    pub tile: String,
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
    if !facts.datatake.is_empty() || !facts.tile.is_empty() {
        return Some(format!(
            "по съёмке|{}|{}|{}|{}",
            facts.platform, facts.datatake, facts.tile, facts.second
        ));
    }
    // Ни того, ни другого — тогда съёмку называет виток вместе с прибором и
    // секундой. Виток длится полтора часа, и один он склеил бы весь пролёт;
    // прибор нужен затем, что на борту их бывает несколько. Так лежит
    // Sentinel-5P: восемь полос радиометра и семь газов над одной и той же
    // пятиминутной гранулой — пятнадцать продуктов с общим витком.
    if !facts.orbit.is_empty() {
        return Some(format!(
            "по витку|{}|{}|{}|{}",
            facts.platform, facts.instrument, facts.orbit, facts.second
        ));
    }
    None
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

    // Тип показываемой части берётся из неё самой, а не из снимка: снимок —
    // это она и есть, но заполняет ей тип сведение (см. [`group`]), и спрашивать
    // о ней надо там, где ответ заведомо есть.
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

/// Слайсы, которые каталог назвал по номеру.
fn slices(products: &[(Facts, DataProduct)]) -> Vec<Slice> {
    products
        .iter()
        .filter(|(facts, _)| !facts.slice.is_empty() && !facts.datatake.is_empty())
        .filter_map(|(facts, _)| {
            Some(Slice {
                datatake: facts.datatake.clone(),
                key: acquisition(facts)?,
                second: facts.second,
            })
        })
        .collect()
}

/// Ключ слайса, рядом с которым снята эта часть. `None` — номер слайса у неё
/// свой, либо рядом такого слайса нет.
///
/// Нужно это затем, что номер слайса каталог сообщает не всем частям: у
/// Sentinel-1 его нет у `OCN`, и без этого хода производная величина уезжала бы
/// в собственный снимок — при том что начинается она в ту же микросекунду, что
/// и снимок, из которого её посчитали.
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
        let key = beside_a_slice(&slices, &facts)
            .or_else(|| acquisition(&facts))
            .unwrap_or_else(|| by_name(&facts, &product.name));
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
                        online: product.online,
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
