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
/// `measured` — файлы, которые манифест продукта назвал измерением (пути от
/// корня продукта; см. `manifest::measurements`). Пусто — манифеста не было
/// или он ничего не сказал, и подробный растр выбирается по именам файлов, как
/// и до него.
pub fn scan(keys: &[String], measured: &[String]) -> Scan {
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

    // Sentinel-1: квиклук в preview/, подробный — measurement-COG. Только
    // продукты -COG: их тайловый GeoTIFF с копиями читается точечно, а
    // полосным гигантам старых GRD подробная роль стоила бы прохода по
    // всему файлу через сеть. Ко-поляризация (vv/hh) предпочтительнее
    // кросс-: на её амплитуде читаются и суша, и море.
    if rasters.is_empty() {
        if let Some(quicklook) = keys.iter().find(|key| key.ends_with("/preview/quick-look.png"))
        {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        let cog = |key: &str| {
            key.contains("_COG.SAFE/")
                && key.contains("/measurement/")
                && (key.ends_with(".tiff") || key.ends_with(".tif"))
        };
        let measurement = keys
            .iter()
            .find(|key| cog(key) && (key.contains("-vv-") || key.contains("-hh-")))
            .or_else(|| keys.iter().find(|key| cog(key)));
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
        // Порядок выбора: сперва то, что похоже на цветной снимок целиком,
        // потом по алфавиту — не выбор, а определённость: одному продукту один
        // и тот же ответ от запуска к запуску.
        let detailed = among
            .into_iter()
            .filter(|key| !a_quicklook(key) && !a_decoration(key))
            .min_by_key(|key| (!a_whole_picture(key), file_name(key)));
        if let Some(detailed) = detailed {
            rasters.push((detailed.clone(), Role::Detailed));
        }
        // Сюда доходят только неузнанные раскладки, и подробный растр здесь
        // выбран именами файлов — то есть догадкой. Ею и отличается случай,
        // ради которого стои́т идти за манифестом.
        return Scan { rasters, guessed: true };
    }

    Scan { rasters, guessed: false }
}

/// Что вышло из [`scan`]: растры с ролями и то, чем выбран подробный.
pub struct Scan {
    pub rasters: Vec<(String, Role)>,
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
fn a_quicklook(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    ["quick-look", "quicklook", "thumb", "browse", "preview", "_pvi"]
        .iter()
        .any(|hint| name.contains(hint))
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
    let mut words = name.split(['_', '-', '.', ' ']).peekable();
    while let Some(word) = words.next() {
        if ["TCI", "TRUECOLOR", "RGB"].contains(&word) {
            return true;
        }
        if word == "TRUE" && words.peek() == Some(&"COLOR") {
            return true;
        }
    }
    false
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
///    (см. `image-tiler::netcdf::seating`);
///  * `geodetic_<сетка>.nc` — поотсчётные координаты той сетки, на которой
///    записан растр (SLSTR держит их по одному файлу на сетку: `_in`, `_an`,
///    `_fn`), — когда опорной в продукте нет;
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
    // Опорная сетка съёмки — первый ответ у всякой сетки SLSTR, и она же самый
    // дешёвый: у гранулы `SL_2_LST` это 394 КБ против 2,2 МБ поотсчётного
    // файла.
    // Платится за это точностью, и цена измерена: узлы `tx` стоят на
    // номинальной решётке прибора и отходят от поотсчётных координат на 453 м
    // медианно, 825 м на девяносто пятом и 1667 м в худшем. Меньше пикселя
    // километрового растра, и это же — пол привязки: сгущать по такому файлу
    // решётку не за чем, ниже собственного смещения она не опустится.
    //
    // Растру, записанному на самой опорной сетке (`met_tx.nc`), тот же файл и
    // достаётся — своей сетки у него нет другой.
    if grid_tag(raster).is_some()
        && let Some(found) = sibling("geodetic_tx.nc")
    {
        return Some(found);
    }
    if let Some(grid) = grid_tag(raster)
        && let Some(found) = sibling(&format!("geodetic_{}.nc", grid))
    {
        return Some(found);
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
pub fn unviewable(identifier: &str, folder: bool, level: Option<u32>) -> String {
    if level == Some(0) {
        return "это сырьё уровня 0: изображения в продукте нет вовсе, только эхо приёмника"
            .to_string();
    }
    match folder || single(identifier).is_some() {
        true => String::new(),
        false => just_a_file(identifier),
    }
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
    fn scan_rasters(keys: &[String], measured: &[String]) -> Vec<(String, Role)> {
        scan(keys, measured).rasters
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
        let rasters = scan_rasters(&keys, &[]);
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
        let rasters = scan_rasters(&keys, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[1].0.ends_with("_TCI.jp2") && rasters[1].1 == Role::Detailed);
    }

    #[test]
    fn sentinel1_plain_grd_yields_quicklook_only() {
        // Не-COG: measurement — полосный гигант, подробной роли не получает.
        let keys = keys(&[
            "eodata/…/S1C_IW_GRDH.SAFE/preview/quick-look.png",
            "eodata/…/S1C_IW_GRDH.SAFE/measurement/s1c-iw-grd-vv.tiff",
            "eodata/…/S1C_IW_GRDH.SAFE/manifest.safe",
        ]);
        let rasters = scan_rasters(&keys, &[]);
        assert_eq!(rasters.len(), 1);
        assert!(rasters[0].0.ends_with("quick-look.png") && rasters[0].1 == Role::Preview);
    }

    #[test]
    fn sentinel1_grd_cog_yields_copol_measurement() {
        let keys = keys(&[
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/preview/quick-look.png",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/measurement/s1c-iw-grd-vh-20260812t153507-002-cog.tiff",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/measurement/s1c-iw-grd-vv-20260812t153507-001-cog.tiff",
            "eodata/…/S1C_IW_GRDH_1SDV_COG.SAFE/manifest.safe",
        ]);
        let rasters = scan_rasters(&keys, &[]);
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
        let rasters = scan_rasters(&clms, &[]);
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
        let rasters = scan_rasters(&bands, &[]);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[0].0.ends_with("_thumb_large.jpg") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("_B1.TIF") && rasters[1].1 == Role::Detailed);
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
        let rasters = scan_rasters(&granule, &[]);
        assert_eq!(rasters.len(), 1);
        assert!(rasters[0].0.ends_with("rgb_TCI.nc"));

        assert!(a_whole_picture("x/T35UNV_20260818_TCI_10m.jp2"));
        assert!(a_whole_picture("x/scene_true_color.tif"));
        assert!(a_whole_picture("x/L2_TRUECOLOR.png"));
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
        let rasters = scan_rasters(&ocn, &[]);
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
        let rasters = scan_rasters(&only, &[]);
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
        let raw = unviewable("eodata/…/S1C_IW_RAW__0SDV.SAFE", true, Some(0));
        assert!(raw.contains("уровня 0"), "отказ не назвал уровень: {}", raw);
        // Архив калибровочных таблиц — один файл, и он не растр. Отказ
        // называет и то, что лежит, и то, что подошло бы.
        let archive = unviewable("eodata/…/S2A_OPER_GIP_R2EQOG_B03.TGZ", false, Some(1));
        assert!(archive.contains(".tgz"), "отказ не назвал файла: {}", archive);
        assert!(archive.contains("TIFF"), "отказ не назвал подходящего: {}", archive);
        // Гранула Sentinel-5P — один файл, и он читается.
        assert!(unviewable("eodata/…/S5P_NRTI_L2__NO2___.nc", false, Some(2)).is_empty());
        // Папка: что внутри, скажет листинг.
        assert!(unviewable("eodata/…/S2C_MSIL2A_T40WFC.SAFE", true, Some(2)).is_empty());
        // Уровень неизвестен (листинг хранилища) — судим по одному ключу.
        assert!(unviewable("eodata/…/tile.TIF", false, None).is_empty());
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

    /// У SLSTR сеток несколько, и опорная общая им всем: растр сетки `in`
    /// привязывается `geodetic_tx.nc` — на порядок более дешёвым, чем
    /// поотсчётный `geodetic_in.nc`.
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
