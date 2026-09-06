//! Растры продукта для наложения: какие файлы каких ролей лежат внутри.
//!
//! Раскладку .SAFE знает только этот модуль — как и раскладку бакета. Роли
//! две, по назначению: превью — маленький файл, дающий наложению картинку
//! сразу; подробный — то, к чему идут на приближении.
//!
//! Разбор идёт от точного к приблизительному. Сперва шаблоны имён известных
//! миссий: там раскладка названа, и гадать не о чем. Не узналась ни одна —
//! решают имена самих файлов, и это уже догадка, но честная: под неузнанные
//! раскладки подпадают Landsat и гранулы Sentinel-3, а показать одну полосу
//! куда полезнее, чем промолчать о продукте, в котором их два десятка.

/// Роль растра. Своя, а не из types.proto: раскладку .SAFE разбирает чистая
/// функция, и знать протокол ей незачем — в роль контракта её переводит
/// match на границе (см. `cdse::imagery_response`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

/// Ключи поддерева продукта → растры с ролями. Ключи — полные идентификаторы
/// (с префиксом бакета), как их отдаёт листинг.
///
/// `product` — идентификатор самого продукта, тот же, которым листали
/// поддерево. Спрашивается он ради одного: продукт называет величину, которую
/// меряет, и по ней среди полос узнаётся измерение (см.
/// [`names_the_measurand`]).
///
/// `measured` — файлы, которые манифест продукта назвал измерением (пути от
/// корня продукта; см. `manifest::measurements`). Пусто — манифеста не было
/// или он ничего не сказал, и подробный растр выбирается по именам файлов, как
/// и до него.
pub fn scan(product: &str, keys: &[String], measured: &[String], downloaded: bool) -> Scan {
    let mut rasters = Vec::new();

    // Sentinel-2: квиклук гранулы лежит в QI_DATA, истинный цвет — в
    // IMG_DATA (L2A — по разрешениям, R10m самый подробный; L1C — плоско).
    if let Some(pvi) = keys.iter().find(|key| key.ends_with("_PVI.jp2")) {
        rasters.push((pvi.clone(), Role::Preview));
    }
    let tci = keys
        .iter()
        .find(|key| key.contains("/IMG_DATA/R10m/") && key.ends_with("_TCI_10m.jp2"))
        .or_else(|| {
            keys.iter().find(|key| key.contains("/IMG_DATA/") && key.ends_with("_TCI.jp2"))
        });
    if let Some(tci) = tci {
        rasters.push((tci.clone(), Role::Detailed));
    }

    // Sentinel-1: квиклук в preview/, подробный — measurement. Ко-поляризация
    // (vv/hh) предпочтительнее кросс-: на её амплитуде читаются и суша, и море.
    //
    // Sentinel-1: квиклук в preview/, подробный — measurement. Ко-поляризация
    // (vv/hh) предпочтительнее кросс-: на её амплитуде читаются и суша, и море.
    //
    // Тайловый GeoTIFF с копиями (-COG) предлагается всегда: у него на всякий
    // уровень своя копия, и любой из них стои́т своих тайлов.
    //
    // А полосный гигант старых GRD — только когда файл под рукой, и решает это
    // не подробный его край, а грубый. Подробный тайл у него дёшев: 512 строк
    // на всю ширину, десятки мегабайт, и тайлер читает такой уровень окном
    // (точечная строка таблицы уровней). А вот грубому уровню нужна КАЖДАЯ
    // строка файла —
    // полосу не пропустишь, нужная строка есть в каждой, — и он стои́т целого
    // прохода. Спрашивают же именно грубый: канва просмотра открывает
    // подробный растр и вписывает его в окно. По сети это четверть часа, с
    // диска — секунды.
    //
    // Знает это только заказчик: что лежит на диске, ведёт библиотека, а
    // провайдер видит одно хранилище.
    if rasters.is_empty() {
        if let Some(quicklook) = keys.iter().find(|key| key.ends_with("/preview/quick-look.png"))
        {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        let raster = |key: &str| {
            key.contains("/measurement/") && (key.ends_with(".tiff") || key.ends_with(".tif"))
        };
        let affordable = |key: &str| raster(key) && (downloaded || key.contains("_COG.SAFE/"));
        let measurement = keys
            .iter()
            .find(|key| affordable(key) && (key.contains("-vv-") || key.contains("-hh-")))
            .or_else(|| keys.iter().find(|key| affordable(key)));
        if let Some(measurement) = measurement {
            rasters.push((measurement.clone(), Role::Detailed));
        }
    }

    // Ни одна раскладка не узналась. Тогда решает то единственное, что о файлах
    // известно до открытия, — их имена.
    //
    // Отказаться было бы проще, но неверно: под это правило подпадают не
    // экзотика, а Landsat со всей его оптикой до Sentinel и гранулы Sentinel-3,
    // где полос два десятка. Показать одну полосу — честный ответ («вот что в
    // продукте есть»), а промолчать — нет.
    if rasters.is_empty() {
        let readable: Vec<&String> = keys.iter().filter(|key| is_raster(key)).collect();
        if let Some(quicklook) = readable.iter().copied().find(|key| a_quicklook(key)) {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        // Названное манифестом сужает выбор до измерения — там, где манифест
        // есть и говорит о читаемом файле. Не сужает до пустоты: измерением
        // бывает и то, что растром не открыть (сырьё уровня 0 записано `.dat`),
        // и тогда вопрос остаётся прежним.
        let named: Vec<&String> =
            readable.iter().copied().filter(|key| measured_by(key, measured)).collect();
        let among = match named.is_empty() {
            true => readable,
            false => named,
        };
        // Порядок выбора: сперва то, что похоже на цветной снимок целиком;
        // потом измерительный формат против показного; потом названное
        // величиной самого продукта; потом всё, кроме объявивших себя
        // подсобными; потом самая густая сетка записи, если имя её называет
        // ([`grid_rank`]); а не различило ни одно из пяти — по алфавиту: это
        // уже не выбор, а определённость, одному продукту один и тот же ответ
        // от запуска к запуску.
        let mut ranked: Vec<&String> =
            among.into_iter().filter(|key| !a_quicklook(key) && !a_decoration(key)).collect();
        ranked.sort_by_key(|key| {
            (
                !a_whole_picture(key),
                a_picture_format(key),
                !names_the_measurand(key, product),
                an_aside(key),
                grid_rank(key),
                file_name(key),
            )
        });
        let mut alternates = Vec::new();
        if let Some((detailed, rest)) = ranked.split_first() {
            rasters.push(((*detailed).clone(), Role::Detailed));
            alternates = spares(detailed, rest);
        }
        // Сюда доходят только неузнанные раскладки, и подробный растр здесь
        // выбран именами файлов — то есть догадкой. Ею и отличается случай,
        // ради которого стои́т идти за манифестом.
        return Scan { rasters, alternates, guessed: true };
    }

    Scan { rasters, alternates: Vec::new(), guessed: false }
}

/// Запасные подробные растры за выбранным: лучший файл каждой сетки грубее
/// его, по порядку ранга ([`grid_rank`]), без опорной и без файлов, чьё имя о
/// сетке молчит — калибровочная таблица прибора запасным растром не годится.
///
/// Выбор по имени не видит, что в файле: у гранулы SLSTR, снятой ночью,
/// видимые каналы записаны сплошным «нет данных», и полукилометровый
/// `S1_radiance_an.nc` — пустой. Тайлер такое отвергает как пустое, и слой
/// остался бы без подробного растра и без его привязки, хотя рядом лежит
/// километровый тепловой канал с данными. Поэтому вместе с выбранным
/// называется, что пробовать следом: по одному на сетку, потому что соседи
/// по сетке пусты по той же причине. Без хвостов сеток (OLCI, Landsat) запасных
/// нет — второго ответа имена не дают.
fn spares<'a>(detailed: &String, rest: &[&'a String]) -> Vec<String> {
    let chosen = grid_rank(detailed).0;
    let mut seen = Vec::new();
    rest.iter()
        .filter(|key| {
            let grid = grid_rank(key).0;
            let fresh = grid_tag(key).is_some() && grid > chosen && grid < 2 && !seen.contains(&grid);
            if fresh {
                seen.push(grid);
            }
            fresh
        })
        .map(|key| (*key).clone())
        .collect()
}

/// Названный смотрящим файл — подробным растром, а выбор раскладки — за ним
/// запасным: смотрящему виднее, какой канал ему нужен, но файл, который не
/// откроется, оставил бы слой без подробного вовсе. Названное не из продукта,
/// не растр по имени (манифест) или квиклук — отказ словами (второй ответ)
/// и растры раскладки: такой выбор не показать ничем, а молча заменить его
/// значило бы соврать строкой слоя. Растр ли файл на самом деле, по имени не
/// видно (`geodetic_*.nc` — тоже `.nc`): это скажет тайлер, описав его.
/// Превью остаётся своё.
pub fn preferring(scan: Scan, wanted: &str, keys: &[String]) -> (Scan, Option<String>) {
    if wanted.is_empty() {
        return (scan, None);
    }
    let refused = if !keys.iter().any(|key| key == wanted) {
        Some("его нет в продукте")
    } else if !is_raster(wanted) {
        Some("по имени это не растр")
    } else if scan.rasters.iter().any(|(key, role)| key == wanted && *role == Role::Preview) {
        Some("это квиклук, он и так лежит превью")
    } else {
        None
    };
    if let Some(why) = refused {
        let said = format!("файл '{}' подробным не лёг: {} — лежит выбор раскладки", file_name(wanted), why);
        log::warn!(target: "handlers", "{}", said);
        return (scan, Some(said));
    }
    let mut rasters: Vec<(String, Role)> =
        scan.rasters.iter().filter(|(_, role)| *role == Role::Preview).cloned().collect();
    rasters.push((wanted.to_string(), Role::Detailed));
    let alternates = scan
        .rasters
        .iter()
        .filter(|(key, role)| *role == Role::Detailed && key != wanted)
        .map(|(key, _)| key.clone())
        .chain(scan.alternates.into_iter().filter(|key| key != wanted))
        .collect();
    (Scan { rasters, alternates, guessed: scan.guessed }, None)
}

/// Что вышло из [`scan`]: растры с ролями, запасные за подробным и то, чем
/// выбран подробный.
pub struct Scan {
    pub rasters: Vec<(String, Role)>,
    /// Подробные растры на случай, если выбранный не откроется или пуст, в
    /// порядке предпочтения (см. [`spares`]).
    pub alternates: Vec<String>,
    /// Подробный растр выбран догадкой по именам файлов, а не раскладкой
    /// известной миссии. Только такому выбору и нужен манифест: у Sentinel-1 и
    /// Sentinel-2 раскладка названа, и лишний подписанный запрос на сотни
    /// килобайт ничего не добавит.
    pub guessed: bool,
}

/// Последний сегмент ключа.
fn file_name(key: &str) -> &str {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Имя, за которым обычно лежит маленькая обзорная картинка, а не измерение.
///
/// Куском имени, потому что пишут это по-разному: и через дефис, и слитно, и с
/// приставкой миссии. Исключение — `BP`: две буквы куском ловят что угодно,
/// поэтому Browse Product узнаётся отдельным словом.
///
/// Зовёт им свою картинку архив ESA: у Landsat-5 рядом с каталогом `.TIFF`, где
/// лежат сами полосы, стои́т `LS05_…_52FE.BP.PNG`. Не названная обзорной, она
/// становится подробным растром — по алфавиту `LS05…` идёт раньше
/// `LT51…_B1.TIF`, — и на шар вместо снимка ложится картинка для показа.
fn a_quicklook(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    let by_piece = ["quick-look", "quicklook", "thumb", "browse", "preview", "_pvi"]
        .iter()
        .any(|hint| name.contains(hint));
    by_piece || words(&name).any(|word| word == "bp")
}

/// Имя, которое само говорит, что файл стои́т при измерении, а не является им.
///
/// Словом, а не куском: «coordinates» внутри чужого имени ничего не значит.
///
/// Слова здесь те же, какими зовутся файлы координат у [`geolocation`], плюс
/// «ancillary». Разница в вопросе: там ищут ровно тот файл, который сядет под
/// растр, здесь — узнаю́т всякий такой, чтобы он не сел на шар вместо растра.
///
/// Держится это на том, что лежит в дереве. У гранулы SLSTR рядом с `LST_in.nc`
/// лежит `LST_ancillary_ds.nc`, и по алфавиту первым стои́т он — прописные буквы
/// идут раньше строчных, так что спор решается на `a` против `i`. У гранулы
/// OLCI строчными названы все, и первым по алфавиту идёт `geo_coordinates.nc` —
/// файл чистых широт с долготами; измерение (`gifapar.nc`) стои́т сразу за ним.
///
/// Обычно такую гранулу разбирает манифест, он измерение называет прямо; но
/// манифест бывает и недоступен (`cdse.rs`, ветка `Asked::Manifest`), и тогда
/// весь ответ — в именах файлов.
fn an_aside(key: &str) -> bool {
    const ASIDE: [&str; 4] = ["ancillary", "coordinates", "geodetic", "geolocation"];
    let name = file_name(key).to_ascii_lowercase();
    words(&name).any(|word| ASIDE.contains(&word))
}

/// Расширения, за которыми приезжает измерение, а не картинка для показа: они
/// умеют и отсчёты шире байта, и метку «нет данных».
///
/// Список один, а не два: спрашивают у того, что уже прошло [`is_raster`], и
/// «не измерительное» там означает ровно «показное».
const MEASURED_SUFFIXES: [&str; 6] = ["tif", "tiff", "jp2", "j2k", "nc", "h5"];

/// Возит ли ключ картинку для показа, а не измерение.
///
/// Расширением, а не именем: у Landsat в архиве ESA обзорная картинка зовётся
/// именем продукта и от полос отличается только им — `LS05_…_52FE.BP.PNG`
/// против `LT51…_B1.TIF`, — а всякое имя такой картинки не перечислить.
/// Измерения в PNG не возят: ни отсчёта шире байта, ни «нет данных» он не
/// умеет.
fn a_picture_format(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((_, suffix)) => !MEASURED_SUFFIXES.contains(&suffix),
        None => true,
    }
}

/// Имя, разобранное на слова: разделители у всех разборщиков имён одни и те же,
/// и разойтись им нельзя — иначе `BP` в одном месте слово, а в другом кусок.
fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(['_', '-', '.', ' ']).filter(|word| !word.is_empty())
}

/// Назван ли ключ манифестом среди измерений. Пути манифеста идут от корня
/// продукта, ключи — от корня бакета, и сходятся они хвостом.
fn measured_by(key: &str, measured: &[String]) -> bool {
    measured.iter().any(|href| key.ends_with(&format!("/{}", href)))
}

/// Лежит ли ключ в подкаталоге показа, а не измерения.
///
/// Каталог `preview/` — это часть раскладки, и сказано в нём то же, что
/// словом: внутри обзорная картинка, цветовая шкала прибора, логотип миссии.
/// Подробным растром там не лежит ничего, а по имени файла это не разобрать:
/// у Sentinel-1 OCN на подробный растр иначе претендует
/// `preview/icons/logo.png` — «logo» стои́т в алфавите раньше «measurement».
fn a_decoration(key: &str) -> bool {
    key.contains("/preview/")
}

/// Имя, за которым обычно лежит цветной снимок целиком, а не одна его полоса.
///
/// Сравнение по слову, а не по куску: «TCI» внутри слова ничего не значит, и
/// подстрокой оно ловит `otci.nc` — индекс хлорофилла OLCI, у которого с
/// цветным снимком общего только три буквы.
fn a_whole_picture(key: &str) -> bool {
    let name = file_name(key).to_ascii_uppercase();
    let mut parts = words(&name).peekable();
    while let Some(word) = parts.next() {
        if ["TCI", "TRUECOLOR", "RGB"].contains(&word) {
            return true;
        }
        if word == "TRUE" && parts.peek() == Some(&"COLOR") {
            return true;
        }
    }
    false
}

/// Названа ли полоса той же величиной, что и сам продукт.
///
/// CLMS складывает имя файла из имени продукта и приставки полосы: у продукта
/// `c_gls_LST_202608271600_GLOBE_GEO_V3.0.1_cog` внутри лежат
/// `c_gls_LST-LST_…`, `c_gls_LST-ERRORBAR_…`, `c_gls_LST-QFLAG_…` и
/// `c_gls_LST-TDELTA_…`, все четыре — читаемые растры одного размера. По
/// алфавиту первой из них стои́т погрешность, и на шар легла бы она — разброс в
/// кельвинах вместо самой температуры, причём отличить одно от другого по виду
/// нельзя: маска «нет данных» у полос общая.
///
/// Повтором слова, а не списком служебных приставок: список пришлось бы вести
/// за всеми продуктами CLMS, а свою величину продукт называет сам. Слово должно
/// встретиться в имени файла **чаще**, чем в имени продукта, — просто
/// вхождения мало: имя продукта целиком лежит в каждой из четырёх полос, и на
/// него отвечали бы все разом.
///
/// Числа словами здесь не считаются: дата и куски версии повторяются в именах
/// полос сами собой, а о величине не говорят ничего.
///
/// Считается по последнему сегменту пути: соседние сегменты приносят в счёт
/// свои слова, и повтор, случившийся выше по дереву хранилища, погасил бы
/// верное срабатывание — величина оказалась бы названа дважды ещё до полосы.
///
/// Повтор дословный, слово в слово. У продукта, названного вместе с
/// разрешением (`c_gls_LAI300_…` при полосе `-LAI`), слова разные, и правило
/// молчит — решает алфавит, как и всюду, где повторять нечего.
///
/// «Нет» здесь — обычный ответ, а не промах: у Landsat полосы зовутся `_B1`…
/// `_B7`, повторять в них нечего, и решает алфавит. Ярусом выше стои́т цветной
/// снимок целиком ([`a_whole_picture`]): он готовая картинка, а повтор величины
/// различает лишь полосы измерения между собой.
fn names_the_measurand(key: &str, product: &str) -> bool {
    let product = file_name(product);
    let times = |name: &str, word: &str| {
        words(name).filter(|other| other.eq_ignore_ascii_case(word)).count()
    };
    words(product)
        .filter(|word| !word.bytes().all(|sign| sign.is_ascii_digit()))
        .any(|word| times(file_name(key), word) > times(product, word))
}

/// Файл с координатами пикселей растра — там, где они лежат отдельно от него.
///
/// Так упакован Sentinel-3: в измерительном `.nc` лежит одна полоса чисел, а
/// широта с долготой — в соседнем файле того же продукта. Без него полосу
/// съёмки некуда класть: контур каталога говорит, какой кусок Земли снят, но
/// не тем, каким пикселем куда.
///
/// Ищется он среди соседей по каталогу и по имени — раскладку продукта знает
/// только этот модуль. Порядок ответов — от точного к дешёвому:
///  * `geodetic_tx.nc` — опорная сетка съёмки, общая всем сеткам SLSTR: на
///    порядок дешевле поотсчётной, а стои́т она в своём отсчёте прибора и
///    садится на растр только через смещения, объявленные обоими файлами
///    (см. `image-tiler::netcdf::seating`). Первый ответ у километровых
///    сеток (`i`, `f`) и у самой опорной;
///  * `geodetic_<сетка>.nc` — поотсчётные координаты той сетки, на которой
///    записан растр (SLSTR держит их по одному файлу на сетку: `_in`, `_an`,
///    `_fn`). Первый ответ у полукилометровых сеток (`a`, `b`, `c`): узлы
///    опорной отходят от поотсчётных на 453 м медианно и 1667 м в худшем —
///    меньше километрового пикселя и один-три полукилометровых, — а привязка
///    грубее растра съедала бы его подробность. У остальных — когда опорной в
///    продукте нет;
///  * `tie_geo_coordinates.nc` — опорная сетка OLCI: у полного разрешения это
///    1,2 МБ против 50 МБ поотсчётного файла, а узлов в ней хватает с
///    запасом (привязка всё равно берётся решёткой);
///  * `geo_coordinates.nc` — поотсчётные координаты OLCI, когда опорной сетки
///    в продукте нет;
///  * `geolocation.nc` — так называет свой файл SYNERGY (`SY_2_SYN`), у
///    которого нет ни одного из имён выше.
///
/// Пусто — координаты либо в самом растре, либо их нет; спрашивать нечего.
pub fn geolocation(keys: &[String], raster: &str) -> Option<String> {
    // Только рядом с растром: у продукта бывает несколько подкаталогов, и
    // координаты чужой сетки хуже, чем никаких.
    let folder = raster.rsplit_once('/')?.0;
    let sibling = |name: &str| {
        let wanted = format!("{}/{}", folder, name);
        keys.iter().find(|key| key.as_str() == wanted).cloned()
    };
    // Опорная сетка съёмки — первый ответ у километровой сетки SLSTR, и она
    // же самый дешёвый: у гранулы `SL_2_LST` это 394 КБ против 2,2 МБ
    // поотсчётного файла. Платится за это точностью, и цена измерена: узлы
    // `tx` стоят на номинальной решётке прибора и отходят от поотсчётных
    // координат на 453 м медианно, 825 м на девяносто пятом и 1667 м в
    // худшем. Меньше пикселя километрового растра, и это же — пол привязки:
    // сгущать по такому файлу решётку не за чем, ниже собственного смещения
    // она не опустится. У полукилометровой сетки то же смещение — один-три
    // пикселя, и ей первым отвечает поотсчётный файл её сетки.
    //
    // Растру, записанному на самой опорной сетке (`met_tx.nc`), тот же файл и
    // достаётся — своей сетки у него нет другой.
    if let Some(grid) = grid_tag(raster) {
        let own = format!("geodetic_{}.nc", grid);
        let order = match grid_rank(raster).0 == 0 {
            true => [own.as_str(), "geodetic_tx.nc"],
            false => ["geodetic_tx.nc", own.as_str()],
        };
        if let Some(found) = order.iter().find_map(|name| sibling(name)) {
            return Some(found);
        }
    }
    sibling("tie_geo_coordinates.nc")
        .or_else(|| sibling("geo_coordinates.nc"))
        .or_else(|| sibling("geolocation.nc"))
}

/// Хвост имени, которым Sentinel-3 называет сетку записи: `LST_in.nc` → `in`,
/// `F1_BT_fn.nc` → `fn`. Две буквы, и обе с закрытым списком значений: первая
/// — сама сетка (`a`, `b`, `c` — радиометрические каналы, `i` — инфракрасная,
/// `f` — пожарная, `t` — опорная), вторая — вид (`n` — надир, `o` — косой,
/// `x` — общая опорная).
///
/// Списками, а не «двумя строчными буквами»: под такое правило подпадают и
/// `LST_ancillary_ds.nc`, и `chl_nn.nc` OLCI, у которых никакой сетки в хвосте
/// нет. Файла координат для них не найдётся и так, но правило, отвечающее «да»
/// не о том, однажды на что-нибудь и наткнётся.
///
/// `None` — имя так не устроено, и о сетке ничего не сказано.
fn grid_tag(key: &str) -> Option<&str> {
    const GRIDS: [char; 6] = ['a', 'b', 'c', 'i', 'f', 't'];
    const VIEWS: [char; 3] = ['n', 'o', 'x'];
    let name = file_name(key);
    let stem = name.strip_suffix(".nc")?;
    let (_, tag) = stem.rsplit_once('_')?;
    let mut sign = tag.chars();
    let (grid, view) = (sign.next()?, sign.next()?);
    let shaped = sign.next().is_none() && GRIDS.contains(&grid) && VIEWS.contains(&view);
    shaped.then_some(tag)
}

/// Ранг сетки записи по её хвосту ([`grid_tag`]): чем меньше, тем гуще —
/// (сетка, обзор). Сетки `a`, `b`, `c` — полкилометра, `i` и `f` — километр,
/// `t` — опорная, реже всех; надирный обзор (`n`) прежде косого (`o`): косой
/// у́же и снят под углом, а общая опорная (`x`) — последней. Имя без хвоста
/// сетки ничего о ней не говорит и считается километровым надирным: ни лучше,
/// ни хуже обычного измерения, чтобы у продукта без таких хвостов (OLCI,
/// Landsat) ранг не решал ничего.
///
/// Мерка эта — про густоту сетки, а не про размер файла: одна сетка занимает
/// у SLSTR три октавы байт, и километровый тепловой канал весит больше
/// полукилометрового видимого; размером густоту не измерить.
fn grid_rank(key: &str) -> (u8, u8) {
    let Some(tag) = grid_tag(key) else { return (1, 0) };
    let mut sign = tag.chars();
    let grid = match sign.next() {
        Some('a' | 'b' | 'c') => 0,
        Some('i' | 'f') => 1,
        _ => 2,
    };
    let view = match sign.next() {
        Some('n') => 0,
        Some('o') => 1,
        _ => 2,
    };
    (grid, view)
}

/// Расширения, за которыми стои́т растр. Перечислены они здесь и только здесь:
/// спрашивают об этом двое — и наложение, и раскладка хранилища
/// (`s3::is_single_object`), — а два списка одного и того же однажды разойдутся
/// молча, и файл-растр покажется папкой без единого файла внутри.
///
/// Набор тот же, что открывает тайлер (`adapters::describe`): у него формат
/// определяется по содержимому, поэтому расширение здесь — только предположение
/// о том, стоит ли вообще открывать.
const RASTER_SUFFIXES: [&str; 12] = [
    "tif", "tiff", "jp2", "j2k", "png", "jpg", "jpeg", "nc", "h5", "gif", "bmp", "webp",
];

/// Похож ли ключ на растр — по одному лишь расширению.
pub fn is_raster(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/');
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    let suffix = suffix.to_ascii_lowercase();
    RASTER_SUFFIXES.contains(&suffix.as_str())
}

/// Продукт лежит в хранилище одним объектом: сам себе растр — если это растр.
///
/// Раскладку тут разбирать не в чем, и решает расширение — единственный
/// случай, когда оно не догадка, а всё, что о продукте вообще известно.
/// Ошибиться им безопасно: формат тайлер всё равно определяет по содержимому
/// и отвечает отказом на непохожее.
pub fn single(identifier: &str) -> Option<(String, Role)> {
    let name = identifier.trim_end_matches('/');
    is_raster(name).then(|| (name.to_string(), Role::Detailed))
}

/// Почему этот продукт нельзя положить на шар. Пусто — можно, точнее «похоже,
/// да».
///
/// Причиной, а не флагом: значок, пропавший без объяснения, оставляет
/// смотрящего гадать, а объяснить есть чем — оба отказа здесь названы словами
/// и отличаются друг от друга. Сырьё уровня 0 — твёрдое нет: изображения в
/// таком продукте нет вовсе, есть эхо приёмника. Продукт-файл виден насквозь:
/// растр он или не растр, говорит расширение. А о папке без листинга сказать
/// нечего, и «похоже, да» тут честнее отказа — раскладок много, листинг стоит
/// запроса, а не найденный в ней растр объяснит [`nothing_here`].
///
/// `level` — уровень обработки, если он известен (в листинге хранилища его
/// сообщить некому: уровень живёт в атрибутах каталога).
/// `kind` — продуктовый тип из каталога (`productType`); пусто там, где его
/// никто не сказал (листинг хранилища знает только пути).
pub fn unviewable(identifier: &str, folder: bool, level: Option<u32>, kind: &str) -> String {
    if level == Some(0) {
        return "это сырьё уровня 0: изображения в продукте нет вовсе, только эхо приёмника"
            .to_string();
    }
    if let Some(why) = hopeless_type(kind) {
        return why.to_string();
    }
    match folder || single(identifier).is_some() {
        true => String::new(),
        false => just_a_file(identifier),
    }
}

/// Продуктовые типы, у которых изображения не будет ни при каком чтении.
///
/// Спрашивается это до скачивания — тем и ценно. Тип продукт объявляет сам, и
/// каталог его уже разобрал; без этого отказ приходит от декодера на первом
/// чанке, то есть после того, как гигабайты уже приехали по проводу, а весь
/// путь до этого выглядел исправным.
///
/// Список короткий нарочно, и коротким его держат не лень, а цена ошибки.
/// Лишнее скачивание человек замечает и больше не повторяет; погашенный значок
/// над тем, что открылось бы, он не заметит никогда — просто решит, что данных
/// нет.
///
/// Гасится поэтому весь ПРОДУКТ, а не растр, и мерка тут своя: годится только
/// то, у чего непоказуема всякая часть. Радар до сжатия апертуры (SLC) сюда не
/// идёт, хотя измерительный растр у него и правда комплексный: рядом лежит
/// квиклук, и он показывается — то есть погасить продукт значило бы отнять
/// единственный способ его увидеть. Комплексный отсчёт ловится там, где он и
/// живёт, — на описании растра (`tiff::sampled`).
fn hopeless_type(kind: &str) -> Option<&'static str> {
    let kind = kind.to_ascii_uppercase();
    if kind.starts_with("AUX") {
        return Some("это служебные данные прибора, а не съёмка");
    }
    // Уровень 1B Sentinel-5P. Уровнем его не поймать: `scene::level` читает
    // последнюю цифру, и «1B» для неё единица, то есть обычный первый уровень.
    //
    // Сказано про раскладку продукта, а не про наше неумение. Измерение здесь
    // трёхмерно — строка × пиксель × канал, — и готовым снимком не лежит
    // нигде. Двумерное в файле есть, но это не сцена: углы наблюдения да
    // таблицы прибора, то есть «откуда смотрели», а не «что сняли». Тайлер
    // берёт из них первую по своему порядку и кладёт на шар карту угла —
    // картинку, которая выглядит данными и данными не является.
    //
    // Срез куба по одному каналу изображением стал бы, и координаты для него
    // рядом лежат. Но какой из сотен каналов — снимок, не говорит никто, а
    // поперёк полосы у первой полосы 77 отсчётов, то есть десятки километров
    // на пиксель. Готовое и полезное из этого куба уже посчитано наземным
    // процессором и лежит уровнем 2 — его и показываем.
    let cube = ["L1B_RA_BD", "L1B_IR_", "L1B_ENG", "L1B_CA"];
    if cube.iter().any(|head| kind.starts_with(head)) {
        return Some(
            "это спектральный куб радиометра: готового снимка в нём нет — \
             измерение идёт по сотням каналов, а двумерны только углы наблюдения",
        );
    }
    None
}

/// Форматы, которые открывает наложение, — словами, для отказа.
///
/// Собираются из того же [`RASTER_SUFFIXES`], по которому растр и отбирают:
/// написанные списком отдельно, эти двое уже разошлись — `h5` отбирался и не
/// назывался, — а показывают их одному человеку с обеих сторон.
fn readable() -> String {
    fn said(suffix: &str) -> &str {
        match suffix {
            "tif" | "tiff" => "TIFF",
            "jp2" | "j2k" => "JPEG 2000",
            "png" => "PNG",
            "jpg" | "jpeg" => "JPEG",
            "nc" => "NetCDF",
            "h5" => "HDF5",
            "gif" => "GIF",
            "bmp" => "BMP",
            "webp" => "WebP",
            other => other,
        }
    }
    let mut names: Vec<&str> = Vec::new();
    for suffix in RASTER_SUFFIXES {
        let name = said(suffix);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

/// Продукт — один файл, и растром он не является. Что за файл — видно по нему
/// самому, и это весь ответ: распаковывать архивы наложение не умеет.
pub fn just_a_file(identifier: &str) -> String {
    let name = file_name(identifier);
    match name.rsplit_once('.') {
        Some((_, suffix)) => format!(
            "продукт лежит одним файлом .{}, а наложить можно растр — {}",
            suffix.to_ascii_lowercase(),
            readable()
        ),
        None => format!("продукт лежит одним файлом, а наложить можно растр — {}", readable()),
    }
}

/// Почему в поддереве продукта не нашлось растров — по тем же ключам, по
/// которым их искали.
///
/// Ответ, а не отговорка: «нет растров» одинаково звучит и над сырьём уровня 0,
/// где изображения ещё нет вовсе, и над снимком незнакомой раскладки, где оно
/// есть и мы его не узнали. Поступают с этим по-разному, и различать их
/// снаружи можно только словами.
pub fn nothing_here(identifier: &str, keys: &[String]) -> String {
    if keys.is_empty() {
        return "в хранилище по этому пути нет ни одного файла".to_string();
    }
    let name = file_name(identifier);
    // Сырьё уровня 0 — эхо приёмника, не изображение: собрать снимок из него
    // может только наземный процессор, и растру в таком продукте взяться
    // неоткуда.
    if name.contains("_RAW__0S") {
        return format!(
            "это сырьё уровня 0: изображения в продукте ещё нет, только эхо-сигналы ({} файлов)",
            keys.len()
        );
    }
    // Раскладка тут ни при чём: узнаваемой её быть уже не обязано — при
    // неузнанной показывается первый читаемый файл. Значит читаемого не
    // нашлось ни одного, и сказать об этом надо тем, что в продукте лежит.
    format!(
        "среди {} файлов продукта нет ни одного растра — лежат {}",
        keys.len(),
        suffixes(keys)
    )
}

/// Расширения ключей списком, по разу каждое. Ими названо содержимое продукта
/// в объяснении: «.dat, .xml» сразу говорит, что снимка там и не бывало.
fn suffixes(keys: &[String]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for key in keys {
        let name = key.rsplit('/').next().unwrap_or("");
        let Some((_, suffix)) = name.rsplit_once('.') else { continue };
        let suffix = format!(".{}", suffix.to_ascii_lowercase());
        if !seen.contains(&suffix) {
            seen.push(suffix);
        }
    }
    match seen.is_empty() {
        true => "файлы без расширений".to_string(),
        false => seen.join(", "),
    }
}

#[cfg(test)]
mod tests {
    /// Растры из [`scan`] — тестам нужны только они.
    /// Растры удалённого продукта — того, что ещё не скачан. Скачанный
    /// спрашивается отдельно, там свой ответ.
    fn scan_rasters(product: &str, keys: &[String], measured: &[String]) -> Vec<(String, Role)> {
        scan(product, keys, measured, false).rasters
    }

    use super::*;

    fn keys(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn sentinel2_l2a_yields_pvi_and_r10m_tci() {
        let keys = keys(&[
            "eodata/…/S2C_MSIL2A_T40WFC.SAFE/GRANULE/L2A_T40WFC_A/QI_DATA/T40WFC_20260812_PVI.jp2",
            "eodata/…/S2C_MSIL2A_T40WFC.SAFE/GRANULE/L2A_T40WFC_A/IMG_DATA/R10m/T40WFC_20260812_TCI_10m.jp2",
            "eodata/…/S2C_MSIL2A_T40WFC.SAFE/GRANULE/L2A_T40WFC_A/IMG_DATA/R60m/T40WFC_20260812_TCI_60m.jp2",
            "eodata/…/S2C_MSIL2A_T40WFC.SAFE/GRANULE/L2A_T40WFC_A/IMG_DATA/R10m/T40WFC_20260812_B08_10m.jp2",
            "eodata/…/S2C_MSIL2A_T40WFC.SAFE/MTD_MSIL2A.xml",
        ]);
        let rasters = scan_rasters("eodata/…/S2C_MSIL2A_T40WFC.SAFE", &keys, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[0].0.ends_with("_PVI.jp2") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("_TCI_10m.jp2") && rasters[1].1 == Role::Detailed);
    }

    #[test]
    fn sentinel2_l1c_falls_back_to_flat_tci() {
        let keys = keys(&[
            "eodata/…/S2B_MSIL1C_T33UUP.SAFE/GRANULE/L1C_T33UUP_A/QI_DATA/T33UUP_PVI.jp2",
            "eodata/…/S2B_MSIL1C_T33UUP.SAFE/GRANULE/L1C_T33UUP_A/IMG_DATA/T33UUP_20260601_TCI.jp2",
            "eodata/…/S2B_MSIL1C_T33UUP.SAFE/GRANULE/L1C_T33UUP_A/IMG_DATA/T33UUP_20260601_B02.jp2",
        ]);
        let rasters = scan_rasters("eodata/…/S2B_MSIL1C_T33UUP.SAFE", &keys, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[1].0.ends_with("_TCI.jp2") && rasters[1].1 == Role::Detailed);
    }

    /// Полосный гигант старых GRD показывается только скачанным, и решает это
    /// его ГРУБЫЙ край, а не подробный. Подробный тайл у него дёшев — 512 строк
    /// на всю ширину, тайлер читает такой уровень окном; а грубому нужна каждая
    /// строка файла, и он стои́т целого прохода. Спрашивают же именно грубый:
    /// канва просмотра открывает подробный растр и вписывает его в окно.
    #[test]
    fn sentinel1_plain_grd_yields_measurement_only_when_downloaded() {
        let keys = keys(&[
            "eodata/…/S1C_IW_GRDH.SAFE/preview/quick-look.png",
            "eodata/…/S1C_IW_GRDH.SAFE/measurement/s1c-iw-grd-vv.tiff",
            "eodata/…/S1C_IW_GRDH.SAFE/manifest.safe",
        ]);

        let remote = scan_rasters("eodata/…/S1C_IW_GRDH.SAFE", &keys, &[]);
        assert_eq!(remote.len(), 1, "по сети предложен проход по всему файлу");
        assert!(remote[0].0.ends_with("quick-look.png") && remote[0].1 == Role::Preview);

        let local = scan("eodata/…/S1C_IW_GRDH.SAFE", &keys, &[], true).rasters;
        assert_eq!(local.len(), 2, "скачанный снимок остался при одном квиклуке");
        assert!(local[0].0.ends_with("quick-look.png") && local[0].1 == Role::Preview);
        assert!(local[1].0.ends_with("-vv.tiff") && local[1].1 == Role::Detailed);
    }

    /// А тайловый COG предлагается независимо от того, скачан он или нет:
    /// читается он точечно, и приближение стои́т своих тайлов, а не файла.
    #[test]
    fn sentinel1_cog_measurement_needs_no_disk() {
        let keys = keys(&[
            "eodata/…/S1C_IW_GRDH_COG.SAFE/preview/quick-look.png",
            "eodata/…/S1C_IW_GRDH_COG.SAFE/measurement/s1c-iw-grd-vv-cog.tiff",
        ]);
        for downloaded in [false, true] {
            let rasters = scan("eodata/…/S1C_IW_GRDH_COG.SAFE", &keys, &[], downloaded).rasters;
            assert_eq!(rasters.len(), 2, "скачан={downloaded}");
            assert!(rasters[1].0.ends_with("-vv-cog.tiff") && rasters[1].1 == Role::Detailed);
        }
    }

    #[test]
    fn sentinel1_grd_cog_yields_copol_measurement() {
        let keys = keys(&[
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/preview/quick-look.png",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/measurement/s1c-iw-grd-vh-20260812t153507-002-cog.tiff",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/measurement/s1c-iw-grd-vv-20260812t153507-001-cog.tiff",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/manifest.safe",
        ]);
        let rasters = scan_rasters("eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE", &keys, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[0].0.ends_with("quick-look.png") && rasters[0].1 == Role::Preview);
        // Из двух поляризаций подробной выбрана ко- (vv), не первая по списку.
        assert!(rasters[1].0.contains("-vv-") && rasters[1].1 == Role::Detailed);
    }

    /// Раскладка незнакомая, а читаемый файл в продукте один — он и есть
    /// снимок. Так лежит климатика: каталог из единственного .nc.
    #[test]
    fn a_lone_readable_file_is_the_product_itself() {
        let clms = keys(&[
            "eodata/CLMS/lst/c_gls_LST_202608161000_GLOBE_GEO_V3.0.1_nc/c_gls_LST.nc",
        ]);
        let rasters =
            scan_rasters("eodata/CLMS/lst/c_gls_LST_202608161000_GLOBE_GEO_V3.0.1_nc", &clms, &[]);
        assert_eq!(rasters.len(), 1);
        assert!(rasters[0].0.ends_with(".nc") && rasters[0].1 == Role::Detailed);
    }

    /// Раскладка не узнана, а читаемых файлов несколько — показывается первый
    /// по алфавиту, и обзорная картинка отдельно от него. Промолчать было бы
    /// хуже: под это правило подпадает вся оптика Landsat.
    #[test]
    fn an_unknown_layout_still_shows_what_it_has() {
        let bands = keys(&[
            "eodata/Landsat-5/LT51780121988065ESA00/LT51780121988065ESA00_thumb_large.jpg",
            "eodata/Landsat-5/LT51780121988065ESA00/LT51780121988065ESA00_B2.TIF",
            "eodata/Landsat-5/LT51780121988065ESA00/LT51780121988065ESA00_B1.TIF",
        ]);
        let rasters = scan_rasters("eodata/Landsat-5/LT51780121988065ESA00", &bands, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[0].0.ends_with("_thumb_large.jpg") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("_B1.TIF") && rasters[1].1 == Role::Detailed);
    }

    /// Обзорная картинка архива ESA зовётся `BP` — Browse Product, — и слово
    /// это перечислено: иначе она становится подробным растром, потому что по
    /// алфавиту `LS05…BP.PNG` стои́т раньше полос `LT51…_B1.TIF` из соседнего
    /// каталога. Раскладка настоящая: так лежит Landsat-5 в бакете CDSE.
    #[test]
    fn a_browse_product_is_a_quicklook_not_the_picture() {
        const PRODUCT: &str = "eodata/Landsat-5/TM/L1G/1988/03/05/LS05_RKSE_TM__GEO_1P_52FE";
        let scene = keys(&[
            "eodata/…/LS05_RKSE_TM__GEO_1P_52FE/LS05_RKSE_TM__GEO_1P_52FE.BP.PNG",
            "eodata/…/LS05_RKSE_TM__GEO_1P_52FE/LS05_RKSE_TM__GEO_1P_52FE.BP.XML",
            "eodata/…/LS05_RKSE_TM__GEO_1P_52FE/LS05_RKSE_TM__GEO_1P_52FE.JPG",
            "eodata/…/LS05_RKSE_TM__GEO_1P_52FE/LS05_RKSE_TM__GEO_1P_52FE.TIFF/LT51780121988065ESA00_B2.TIF",
            "eodata/…/LS05_RKSE_TM__GEO_1P_52FE/LS05_RKSE_TM__GEO_1P_52FE.TIFF/LT51780121988065ESA00_B1.TIF",
        ]);
        let rasters = scan_rasters(PRODUCT, &scene, &[]);
        assert_eq!(rasters.len(), 2, "{:?}", rasters);
        assert!(rasters[0].0.ends_with(".BP.PNG") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("_B1.TIF") && rasters[1].1 == Role::Detailed);

        // Словом, а не куском: две буквы подстрокой ловят что угодно.
        assert!(a_quicklook("x/scene.BP.PNG"));
        assert!(!a_quicklook("x/S3A_SL_2_LST_bpm_in.nc"));

        // Второй картинки в списке довольно, чтобы слово `BP` перестало
        // помогать: `LS05_…52FE.JPG` не названо ничем и по алфавиту стои́т
        // раньше полос (`S` < `T`). Различает их формат, а не имя.
        assert!(a_picture_format("x/LS05_RKSE_TM__GEO_1P_52FE.JPG"));
        assert!(!a_picture_format("x/LT51780121988065ESA00_B1.TIF"));
        assert!(!a_picture_format("x/LST_in.nc"));
    }

    /// Гранула OLCI без манифеста: первым по алфавиту стои́т файл координат, а
    /// не измерение — у неё все имена строчные, и уступки регистра, что
    /// выручает SLSTR, здесь нет. Имена настоящие, с гранулы `OL_2_LRR`.
    ///
    /// Те же координаты, найденные [`geolocation`], садятся потом под растр —
    /// одно и то же знание, спрошенное с двух сторон.
    #[test]
    fn a_coordinate_file_never_becomes_the_measurement() {
        const PRODUCT: &str = "eodata/…/S3A_OL_2_LRR____20260824T161836_PS1_O_NR_003.SEN3";
        let granule = keys(&[
            "eodata/…/S3A_OL_2_LRR.SEN3/quicklook.jpg",
            "eodata/…/S3A_OL_2_LRR.SEN3/geo_coordinates.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/gifapar.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/iwv.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/otci.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/tie_geo_coordinates.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/time_coordinates.nc",
            "eodata/…/S3A_OL_2_LRR.SEN3/xfdumanifest.xml",
        ]);
        let rasters = scan_rasters(PRODUCT, &granule, &[]);
        assert_eq!(rasters.len(), 2, "{:?}", rasters);
        assert!(rasters[1].0.ends_with("/gifapar.nc"), "{:?}", rasters[1]);

        // То же самое говорит и манифест — правило с ним не спорит, а
        // повторяет его там, где его нет.
        let said = vec!["gifapar.nc".to_string()];
        assert!(scan_rasters(PRODUCT, &granule, &said)[1].0.ends_with("/gifapar.nc"));
    }

    /// Гранула SLSTR без манифеста: рядом с измерением лежит подсобный файл, и
    /// по алфавиту он первый — прописные идут раньше строчных, так что спор
    /// решается на `a` против `i`. Имена настоящие, с гранулы `SL_2_LST`.
    ///
    /// Обычно такую гранулу разбирает манифест — он измерение называет прямо, —
    /// но манифест бывает и недоступен, и тогда весь ответ в именах файлов.
    #[test]
    fn an_ancillary_file_never_becomes_the_measurement() {
        const PRODUCT: &str = "eodata/…/S3A_SL_2_LST____20260824T174507_0540_PS1_O_NR_005.SEN3";
        let granule = keys(&[
            "eodata/…/S3A_SL_2_LST.SEN3/quicklook.jpg",
            "eodata/…/S3A_SL_2_LST.SEN3/LST_ancillary_ds.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/LST_in.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/flags_in.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/geodetic_in.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/xfdumanifest.xml",
        ]);
        let rasters = scan_rasters(PRODUCT, &granule, &[]);
        assert_eq!(rasters.len(), 2, "{:?}", rasters);
        assert!(rasters[0].0.ends_with("quicklook.jpg") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("/LST_in.nc") && rasters[1].1 == Role::Detailed);

        // А названный манифестом побеждает и без всяких ярусов.
        let said = vec!["LST_ancillary_ds.nc".to_string()];
        let by_manifest = scan_rasters(PRODUCT, &granule, &said);
        assert!(by_manifest[1].0.ends_with("LST_ancillary_ds.nc"), "{:?}", by_manifest);
    }

    /// Полосы CLMS — измерение, погрешность, флаги качества и время съёмки —
    /// все четыре читаемые растры одного размера, и первой по алфавиту стои́т
    /// погрешность: на шар легла бы она — разброс в кельвинах вместо самой
    /// температуры, а по виду это неотличимо, маска «нет данных» у полос общая.
    ///
    /// Спасает то, что имя файла у CLMS — это имя продукта с приставкой полосы:
    /// измерение узнаётся тем, что повторяет величину продукта.
    #[test]
    fn a_clms_product_shows_its_measurement_not_its_error_bar() {
        const PRODUCT: &str = "eodata/…/c_gls_LST_202608271600_GLOBE_GEO_V3.0.1_cog";
        let band = |what: &str| {
            format!("{}/c_gls_LST-{}_202608271600_GLOBE_GEO_V3.0.1.tiff", PRODUCT, what)
        };
        let bands: Vec<String> =
            ["ERRORBAR", "LST", "QFLAG", "TDELTA"].iter().map(|what| band(what)).collect();

        let rasters = scan_rasters(PRODUCT, &bands, &[]);
        assert_eq!(rasters.len(), 1, "{:?}", rasters);
        assert_eq!(rasters[0], (band("LST"), Role::Detailed));

        // Повтор считается по последнему сегменту пути, а не по всему: путь
        // назвал величину ещё до полосы, и со всем путём в счёте `LST` вышло бы
        // два против двух — измерение сравнялось бы с соседями.
        let deep = format!("eodata/CLMS/x_lst_y/2026/08/27/{}", file_name(PRODUCT));
        assert!(names_the_measurand(&band("LST"), &deep));
        assert!(!names_the_measurand(&band("ERRORBAR"), &deep));

        // Дата и версия повторяются в именах полос сами собой, и словами не
        // считаются: иначе на правило отвечала бы всякая полоса, у которой в
        // хвосте стои́т лишний номер.
        let numbered =
            format!("{}/c_gls_LST-QFLAG_202608271600_GLOBE_GEO_V3.0.1-1.tiff", PRODUCT);
        assert!(!names_the_measurand(&numbered, PRODUCT));

        // Полосе, которая величину продукта не повторяет, правило не отвечает
        // ничего, и выбор остаётся за алфавитом — как у Landsat, где полосы
        // зовутся `_B1`…`_B7`.
        let scene = "eodata/Landsat-5/LT51780121988065ESA00";
        assert!(!names_the_measurand(&format!("{}/LT51780121988065ESA00_B1.TIF", scene), scene));
    }

    /// Цветной снимок целиком идёт вперёд отдельных полос — по имени, потому
    /// что до открытия о файлах больше ничего и не известно. Слово целиком, а
    /// не кусок: `otci.nc` — индекс хлорофилла, а не цветной снимок.
    #[test]
    fn a_true_colour_file_wins_over_single_bands() {
        let granule = keys(&[
            "eodata/Sentinel-3/SR/x.SEN3/Oa01_radiance.nc",
            "eodata/Sentinel-3/SR/x.SEN3/rgb_TCI.nc",
        ]);
        let rasters = scan_rasters("eodata/Sentinel-3/SR/x.SEN3", &granule, &[]);
        assert_eq!(rasters.len(), 1);
        assert!(rasters[0].0.ends_with("rgb_TCI.nc"));

        assert!(a_whole_picture("x/T35UNV_20260818_TCI_10m.jp2"));
        assert!(a_whole_picture("x/scene_true_color.tif"));
        assert!(a_whole_picture("x/L2_TRUECOLOR.png"));
        // Пустые токены разборщик отбрасывает, и двойной разделитель пару не
        // разрывает: у ESA двойное подчёркивание — обычное дело.
        assert!(a_whole_picture("x/S3A_OL__TRUE__COLOR.png"));
        assert!(!a_whole_picture("eodata/…/S3A_OL_2_LFR.SEN3/otci.nc"));
        assert!(!a_whole_picture("eodata/…/S3A_OL_2_LFR.SEN3/rc_gifapar.nc"));
    }

    /// Подкаталог `preview/` — часть раскладки, и в нём лежит показ: обзорная
    /// картинка, шкала прибора, логотип миссии. Подробным растром оттуда не
    /// берётся ничего — иначе у Sentinel-1 OCN на шар ложится логотип
    /// Copernicus, а измерение остаётся неоткрытым.
    #[test]
    fn a_logo_never_becomes_the_detailed_raster() {
        let ocn = keys(&[
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/preview/icons/logo.png",
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/preview/quick-look-l2-owi.png",
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/preview/owi-colorbar.png",
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/measurement/s1c-ew-ocn-hh-20260818t193953-001.nc",
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/measurement/s1c-ew1-osw-hh-20260818t193953-002.nc",
            "eodata/…/S1C_EW_OCN__2SDH.SAFE/manifest.safe",
        ]);
        let rasters = scan_rasters("eodata/…/S1C_EW_OCN__2SDH.SAFE", &ocn, &[]);
        assert_eq!(rasters.len(), 2, "{:?}", rasters);
        assert!(
            rasters[0].0.ends_with("quick-look-l2-owi.png") && rasters[0].1 == Role::Preview,
            "{:?}", rasters[0]
        );
        assert!(
            rasters[1].0.ends_with("-ocn-hh-20260818t193953-001.nc")
                && rasters[1].1 == Role::Detailed,
            "{:?}", rasters[1]
        );
    }

    /// Растр лежит только в `preview/` — тогда у продукта есть обзорная
    /// картинка и нет подробного. Это ответ, а не отказ: показать её честнее,
    /// чем промолчать.
    #[test]
    fn a_product_with_only_a_quicklook_still_has_one() {
        let only = keys(&[
            "eodata/…/X.SAFE/preview/quick-look.png",
            "eodata/…/X.SAFE/preview/icons/logo.png",
            "eodata/…/X.SAFE/annotation/report.xml",
        ]);
        let rasters = scan_rasters("eodata/…/X.SAFE", &only, &[]);
        assert_eq!(rasters.len(), 1, "{:?}", rasters);
        assert!(rasters[0].0.ends_with("quick-look.png") && rasters[0].1 == Role::Preview);
    }

    /// Список читаемого — один: чем отбирают растр, тем и называют форматы в
    /// отказе. Разойдясь, они говорят одному человеку разное.
    #[test]
    fn the_refusal_names_what_the_scan_accepts() {
        let said = readable();
        // Каждое расширение, которое отбирает `is_raster`, названо в отказе —
        // своим словом, а не самим суффиксом. Иначе человек читает «открываются
        // …», не находит там своего файла и решает, что формат не тот.
        for suffix in RASTER_SUFFIXES {
            assert!(is_raster(&format!("x/y.{}", suffix)), "{} не отбирается", suffix);
            let named = readable().to_ascii_lowercase();
            assert!(
                named.contains(&suffix.to_ascii_lowercase())
                    || ["tif", "tiff", "jp2", "j2k", "jpg", "jpeg", "nc", "h5"].contains(&suffix),
                "{} не назван в «{}»",
                suffix,
                said
            );
        }
        // А у тех, чьё слово с суффиксом не совпадает, названо само слово.
        for word in ["TIFF", "JPEG 2000", "JPEG", "NetCDF", "HDF5"] {
            assert!(said.contains(word), "{} не назван: {}", word, said);
        }
        assert!(said.contains("HDF5"), "h5 отбирается, а не назван: {}", said);
        assert!(said.contains("NetCDF") && said.contains("JPEG 2000") && said.contains("WebP"));
        // Синонимы названы по разу: «TIFF» не повторяется за tif и tiff.
        assert_eq!(said.matches("TIFF").count(), 1, "{}", said);
        assert_eq!(said.matches("JPEG 2000").count(), 1, "{}", said);
    }

    /// Продукт-файл: растр он или нет, видно по нему самому.
    #[test]
    fn single_object_is_its_own_raster_when_it_is_one() {
        assert_eq!(
            single("eodata/CLMS/dem/tile.TIF"),
            Some(("eodata/CLMS/dem/tile.TIF".to_string(), Role::Detailed))
        );
        assert_eq!(single("eodata/Sentinel-2/AUX/GIP_R2ABCA/S2A_OPER_B00.TGZ"), None);
        assert!(single("eodata/Sentinel-5P/TROPOMI/L2__NO2___/S5P_NRTI.nc").is_some());
    }

    /// Значок «на глобус» обещает показ, поэтому «нет» здесь твёрдое, а «да» —
    /// «похоже»: у папки содержимое неизвестно до листинга.
    #[test]
    fn only_a_definite_no_takes_the_globe_away() {
        // Сырьё уровня 0 — изображения в нём нет, сколько ни листай. И отказ
        // называет именно уровень: по «нет растра» его не отличить от снимка
        // незнакомой раскладки, а поступают с ними по-разному.
        let raw = unviewable("eodata/…/S1C_IW_RAW__0SDV.SAFE", true, Some(0), "IW_RAW__0S");
        assert!(raw.contains("уровня 0"), "отказ не назвал уровень: {}", raw);
        // Архив калибровочных таблиц — один файл, и он не растр. Отказ
        // называет и то, что лежит, и то, что подошло бы.
        let archive = unviewable("eodata/…/S2A_OPER_GIP_R2EQOG_B03.TGZ", false, Some(1), "");
        assert!(archive.contains(".tgz"), "отказ не назвал файла: {}", archive);
        assert!(archive.contains("TIFF"), "отказ не назвал подходящего: {}", archive);
        // Гранула Sentinel-5P — один файл, и он читается.
        assert!(unviewable("eodata/…/S5P_NRTI_L2__NO2___.nc", false, Some(2), "L2__NO2___").is_empty());
        // Папка: что внутри, скажет листинг.
        assert!(unviewable("eodata/…/S2C_MSIL2A_T40WFC.SAFE", true, Some(2), "S2MSI2A").is_empty());
        // Уровень неизвестен (листинг хранилища) — судим по одному ключу.
        assert!(unviewable("eodata/…/tile.TIF", false, None, "").is_empty());

        // Служебные данные гаснут типом продукта — до скачивания. Папкой, а
        // не файлом: у файла отказ дало бы и расширение, и проверка вышла бы
        // пустой.
        let aux = unviewable("eodata/…/S1A_AUX_PP1.SAFE", true, None, "AUX_PP1");
        assert!(aux.contains("служебные"), "AUX обязан гаснуть типом: {aux}");
        assert!(
            unviewable("eodata/…/S1A_AUX_PP1.SAFE", true, None, "").is_empty(),
            "без типа гасить нечем — проверка обязана держаться на нём"
        );

        // Спектральный куб Sentinel-5P: уровень у него первый, и правилом
        // уровня 0 он не ловится — гасит его тип продукта.
        for kind in ["L1B_RA_BD1", "L1B_RA_BD8", "L1B_IR_SIR", "L1B_IR_UVN", "L1B_ENG_DB"] {
            let cube = unviewable("eodata/…/S5P_NRTI.nc", false, Some(1), kind);
            assert!(cube.contains("куб"), "тип {kind} обязан гаснуть: {cube}");
        }

        // А снимаемое типом не гасится, каким бы длинным он ни был: цена
        // ошибки здесь односторонняя. SLC в этом списке не случайно — растр у
        // него комплексный, но квиклук рядом показывается.
        // `L2__*` того же спутника в этом списке нарочно: гаснет уровень 1B, а
        // не миссия, и соседняя буква в типе решает всё.
        for kind in
            ["IW_GRDH_1S", "S2MSI1C", "SL_2_LST___", "L2__CO____", "IW_SLC__1S", "L2__O3__PR"]
        {
            assert!(
                unviewable("eodata/…/product.SAFE", true, Some(2), kind).is_empty(),
                "тип {kind} погашен, а он показуемый"
            );
        }
    }

    /// Пустой ответ объясняется тем, что в продукте лежит: сырьё уровня 0 и
    /// незнакомая раскладка — разные ответы, и путать их нельзя.
    #[test]
    fn empty_answer_names_what_is_inside() {
        let raw = keys(&[
            "eodata/…/S1C_IW_RAW__0SDV_20260816T050356_009016_0C2C.SAFE/manifest.safe",
            "eodata/…/S1C_IW_RAW__0SDV_20260816T050356_009016_0C2C.SAFE/s1c-iw-raw-s-vv.dat",
        ]);
        let said = nothing_here(
            "eodata/…/S1C_IW_RAW__0SDV_20260816T050356_009016_0C2C.SAFE",
            &raw,
        );
        assert!(said.contains("уровня 0"), "{}", said);

        let unknown = keys(&["eodata/…/X.SEN3/report.xml", "eodata/…/X.SEN3/data.dat"]);
        let said = nothing_here("eodata/…/X.SEN3", &unknown);
        assert!(said.contains(".xml") && said.contains(".dat"), "содержимое названо: {}", said);
        assert!(!said.contains("уровня 0"), "{}", said);

        // Листинг пустой — это про хранилище, а не про раскладку.
        assert!(nothing_here("eodata/…/X.SAFE", &[]).contains("ни одного файла"));
    }

    /// Sentinel-3 держит координаты отдельным файлом, и его-то и надо назвать.
    /// У OLCI это опорная сетка — полмегабайта против тридцати мегабайт
    /// поотсчётного файла, а решётка привязки всё равно берётся по узлам.
    #[test]
    fn olci_points_at_the_cheap_tie_grid() {
        let olci: Vec<String> = [
            "eodata/…/S3A_OL_2_LFR.SEN3/gifapar.nc",
            "eodata/…/S3A_OL_2_LFR.SEN3/geo_coordinates.nc",
            "eodata/…/S3A_OL_2_LFR.SEN3/tie_geo_coordinates.nc",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        assert_eq!(
            geolocation(&olci, &olci[0]),
            Some("eodata/…/S3A_OL_2_LFR.SEN3/tie_geo_coordinates.nc".to_string())
        );
        // Опорной сетки нет — остаются поотсчётные координаты.
        assert_eq!(
            geolocation(&olci[..2], &olci[0]),
            Some("eodata/…/S3A_OL_2_LFR.SEN3/geo_coordinates.nc".to_string())
        );
    }

    /// Названный файл ложится подробным, выбор раскладки уходит за ним
    /// запасным, превью остаётся; названное не из продукта или не растр —
    /// как не названное.
    #[test]
    fn a_wanted_file_lies_detailed_and_the_layouts_choice_becomes_a_spare() {
        let keys: Vec<String> = ["p/quicklook.jpg", "p/S1_radiance_an.nc", "p/F2_BT_in.nc", "p/xfdumanifest.xml"]
            .iter()
            .map(|key| key.to_string())
            .collect();
        let scan = || Scan {
            rasters: vec![("p/quicklook.jpg".into(), Role::Preview), ("p/S1_radiance_an.nc".into(), Role::Detailed)],
            alternates: vec!["p/F1_BT_fn.nc".into()],
            guessed: true,
        };
        let (chosen, refused) = preferring(scan(), "p/F2_BT_in.nc", &keys);
        assert_eq!(chosen.rasters, vec![("p/quicklook.jpg".to_string(), Role::Preview), ("p/F2_BT_in.nc".to_string(), Role::Detailed)]);
        assert_eq!(chosen.alternates, vec!["p/S1_radiance_an.nc".to_string(), "p/F1_BT_fn.nc".to_string()]);
        assert!(chosen.guessed && refused.is_none());

        let (same, _) = preferring(scan(), "p/S1_radiance_an.nc", &keys);
        assert_eq!(same.rasters, scan().rasters);
        assert_eq!(same.alternates, vec!["p/F1_BT_fn.nc".to_string()], "названный выбор раскладки не стал своим же запасным");
        // Отказ — словами и растрами раскладки: манифест, чужой файл, квиклук.
        for (wanted, word) in [("p/xfdumanifest.xml", "не растр"), ("q/other.nc", "нет в продукте"), ("p/quicklook.jpg", "квиклук")] {
            let (fallback, refused) = preferring(scan(), wanted, &keys);
            assert_eq!(fallback.rasters, scan().rasters, "{wanted} лёг растром");
            assert!(refused.as_deref().is_some_and(|said| said.contains(word)), "{wanted}: {refused:?}");
        }
        let (plain, refused) = preferring(scan(), "", &keys);
        assert_eq!((plain.alternates, refused), (scan().alternates, None));
    }

    /// Гранула SLSTR уровня 1: измерений двадцать восемь, и первые четыре
    /// довода у всех одинаковы — решает густота сетки, а не алфавит.
    /// Полукилометровый видимый канал надирного обзора берёт верх над
    /// километровым тепловым, который стои́т раньше по алфавиту; косой обзор
    /// той же сетки — после надирного. Имена настоящие, с гранулы `SL_1_RBT`.
    #[test]
    fn the_densest_grid_wins_among_the_swath_measurements() {
        const PRODUCT: &str = "eodata/…/S3A_SL_1_RBT____20260824T174507_0540_PS1_O_NR_005.SEN3";
        let names = [
            "quicklook.jpg", "F1_BT_fn.nc", "F1_BT_fo.nc", "F1_BT_in.nc", "F1_BT_io.nc",
            "F2_BT_in.nc", "S1_radiance_an.nc", "S1_radiance_ao.nc", "S4_radiance_bn.nc",
            "S5_radiance_cn.nc", "S7_BT_in.nc", "S8_BT_in.nc", "S9_BT_io.nc", "met_tx.nc",
            "cartesian_an.nc", "flags_an.nc", "indices_an.nc", "time_an.nc", "geodetic_an.nc",
            "geodetic_tx.nc", "viscal.nc", "xfdumanifest.xml",
        ];
        let granule: Vec<String> = names.iter().map(|name| format!("eodata/…/S3A_SL_1_RBT.SEN3/{name}")).collect();
        let rasters = scan_rasters(PRODUCT, &granule, &[]);
        assert_eq!(rasters.len(), 2, "{:?}", rasters);
        assert!(rasters[0].0.ends_with("quicklook.jpg") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("/S1_radiance_an.nc"), "{:?}", rasters[1]);

        // Запасной — лучший файл сетки грубее: километровый надир; опорная
        // сетка запасным не идёт.
        let found = scan(PRODUCT, &granule, &[], false);
        assert_eq!(found.alternates, vec![format!("eodata/…/S3A_SL_1_RBT.SEN3/F1_BT_fn.nc")]);

        // Названные манифестом измерения — те же двадцать восемь; ответ тот же.
        let measured: Vec<String> = names
            .iter()
            .filter(|name| name.contains("radiance") || name.contains("_BT_"))
            .map(|name| name.to_string())
            .collect();
        assert!(scan_rasters(PRODUCT, &granule, &measured)[1].0.ends_with("/S1_radiance_an.nc"));

        // Косой обзор той же полукилометровой сетки берёт верх над километровым
        // надиром: сетка прежде обзора. Без полукилометровых сеток вовсе —
        // километровый надир, по алфавиту среди равных.
        let half_km = |key: &String| ["_an.nc", "_ao.nc", "_bn.nc", "_bo.nc", "_cn.nc", "_co.nc"].iter().any(|tail| key.ends_with(tail));
        let nadir_half_km = |key: &String| ["_an.nc", "_bn.nc", "_cn.nc"].iter().any(|tail| key.ends_with(tail));
        let no_nadir: Vec<String> = granule.iter().filter(|key| !nadir_half_km(key)).cloned().collect();
        assert!(scan_rasters(PRODUCT, &no_nadir, &[])[1].0.ends_with("/S1_radiance_ao.nc"), "{:?}", scan_rasters(PRODUCT, &no_nadir, &[]));
        let coarse: Vec<String> = granule.iter().filter(|key| !half_km(key)).cloned().collect();
        assert!(scan_rasters(PRODUCT, &coarse, &[])[1].0.ends_with("/F1_BT_fn.nc"), "{:?}", scan_rasters(PRODUCT, &coarse, &[]));
        assert_eq!(grid_rank("x/S1_radiance_an.nc"), (0, 0));
        assert_eq!(grid_rank("x/S1_radiance_ao.nc"), (0, 1));
        assert_eq!(grid_rank("x/F1_BT_fn.nc"), (1, 0));
        assert_eq!(grid_rank("x/met_tx.nc"), (2, 2));
        assert_eq!(grid_rank("x/gifapar.nc"), (1, 0), "имя без сетки — километровый надир");

        // У гранулы без хвостов сеток запасных нет: второго ответа имена не дают.
        let olci: Vec<String> = ["quicklook.jpg", "gifapar.nc", "iwv.nc", "otci.nc"]
            .iter()
            .map(|name| format!("eodata/…/S3A_OL_2_LRR.SEN3/{name}"))
            .collect();
        assert!(scan("eodata/…/S3A_OL_2_LRR.SEN3", &olci, &[], false).alternates.is_empty());
    }

    /// У SLSTR сеток несколько, и опорная общая им всем: растр километровой
    /// сетки `in` привязывается `geodetic_tx.nc` — на порядок более дешёвым,
    /// чем поотсчётный `geodetic_in.nc`, — а полукилометровой `an` первым
    /// отвечает её поотсчётный файл: опорная сетка сдвинула бы его на пиксели.
    #[test]
    fn slstr_takes_the_cheap_tie_grid_of_the_swath() {
        let slstr: Vec<String> = [
            "eodata/…/S3A_SL_2_LST.SEN3/LST_in.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/geodetic_in.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/geodetic_tx.nc",
            "eodata/…/S3A_SL_2_LST.SEN3/geo_coordinates.nc",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        assert_eq!(
            geolocation(&slstr, &slstr[0]),
            Some("eodata/…/S3A_SL_2_LST.SEN3/geodetic_tx.nc".to_string())
        );
        // Растру самой опорной сетки достаётся тот же файл: своей сетки у него
        // нет другой.
        assert_eq!(
            geolocation(&slstr, "eodata/…/S3A_SL_2_LST.SEN3/met_tx.nc"),
            Some("eodata/…/S3A_SL_2_LST.SEN3/geodetic_tx.nc".to_string())
        );
        // Имя не сеточное — опорная сетка ему не достаётся: под правило
        // подпадали бы и служебный `LST_ancillary_ds.nc`, и `chl_nn.nc` OLCI.
        assert_eq!(
            geolocation(&slstr, "eodata/…/S3A_SL_2_LST.SEN3/LST_ancillary_ds.nc"),
            Some("eodata/…/S3A_SL_2_LST.SEN3/geo_coordinates.nc".to_string())
        );
        // Опорной в продукте нет — остаются поотсчётные координаты своей
        // сетки, а не первый попавшийся файл координат.
        let lean: Vec<String> = slstr.iter().filter(|key| !key.ends_with("geodetic_tx.nc")).cloned().collect();
        assert_eq!(
            geolocation(&lean, &lean[0]),
            Some("eodata/…/S3A_SL_2_LST.SEN3/geodetic_in.nc".to_string())
        );
        // Полукилометровая сетка берёт свой поотсчётный файл прежде опорной, а
        // без него — опорную.
        let rbt: Vec<String> = ["S1_radiance_an.nc", "geodetic_an.nc", "geodetic_tx.nc"]
            .iter()
            .map(|name| format!("eodata/…/S3A_SL_1_RBT.SEN3/{name}"))
            .collect();
        assert_eq!(geolocation(&rbt, &rbt[0]), Some(rbt[1].clone()));
        assert_eq!(geolocation(&rbt[..1].iter().chain(&rbt[2..]).cloned().collect::<Vec<_>>(), &rbt[0]), Some(rbt[2].clone()));
        // Хвост из двух букв сеткой ещё не делает: так кончаются и служебный
        // `LST_ancillary_ds.nc`, и `chl_nn.nc` у OLCI.
        assert_eq!(grid_tag("x/LST_ancillary_ds.nc"), None);
        assert_eq!(grid_tag("x/chl_nn.nc"), None);
        assert_eq!(grid_tag("x/F1_BT_fn.nc"), Some("fn"));
        assert_eq!(grid_tag("x/geodetic_tx.nc"), Some("tx"));
    }

    /// У SYNERGY файл координат называется своим третьим именем, и знать о нём
    /// больше некому: ни одного из имён OLCI и SLSTR в продукте нет.
    #[test]
    fn synergy_calls_its_coordinates_by_a_third_name() {
        let syn: Vec<String> = [
            "eodata/…/S3B_SY_2_SYN.SEN3/Syn_Oa01_reflectance.nc",
            "eodata/…/S3B_SY_2_SYN.SEN3/geolocation.nc",
            "eodata/…/S3B_SY_2_SYN.SEN3/tiepoints_olci.nc",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        assert_eq!(
            geolocation(&syn, &syn[0]),
            Some("eodata/…/S3B_SY_2_SYN.SEN3/geolocation.nc".to_string())
        );
    }

    /// Координаты берутся только у соседей по каталогу: у продукта бывает
    /// несколько подкаталогов, и чужая сетка хуже, чем никакой.
    #[test]
    fn coordinates_come_from_the_same_folder() {
        let keys: Vec<String> = [
            "eodata/…/product.SEN3/measurement/band.nc",
            "eodata/…/product.SEN3/geo_coordinates.nc",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        assert_eq!(geolocation(&keys, &keys[0]), None);
        // И у растра, который координат не просит, соседей не ищут вовсе.
        let s2 = vec!["eodata/…/GRANULE/IMG_DATA/T36UXV_TCI_10m.jp2".to_string()];
        assert_eq!(geolocation(&s2, &s2[0]), None);
    }
}
