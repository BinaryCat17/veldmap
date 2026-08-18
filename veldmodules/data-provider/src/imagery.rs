//! Растры продукта для наложения: какие файлы каких ролей лежат внутри.
//!
//! Раскладку .SAFE знает только этот модуль — как и раскладку бакета. Роли
//! две, по назначению: превью — маленький файл, дающий наложению картинку
//! сразу; подробный — то, к чему идут на приближении. Выбор по шаблонам имён
//! миссий; продукт без узнаваемых растров — честный пустой список, а не
//! догадки по расширениям.

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
pub fn scan(keys: &[String]) -> Vec<(String, Role)> {
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
        // Порядок выбора: сперва то, что похоже на цветной снимок целиком,
        // потом по алфавиту — не выбор, а определённость: одному продукту один
        // и тот же ответ от запуска к запуску.
        let detailed = readable
            .into_iter()
            .filter(|key| !a_quicklook(key))
            .min_by_key(|key| (!a_whole_picture(key), file_name(key)));
        if let Some(detailed) = detailed {
            rasters.push((detailed.clone(), Role::Detailed));
        }
    }

    rasters
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

/// Имя, за которым обычно лежит цветной снимок целиком, а не одна его полоса.
fn a_whole_picture(key: &str) -> bool {
    let name = file_name(key).to_ascii_uppercase();
    ["TCI", "TRUE_COLOR", "TRUECOLOR", "_RGB"].iter().any(|hint| name.contains(hint))
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

/// Стои́т ли предлагать этот продукт положить на шар.
///
/// Ответов честных два, и «похоже, да» — один из них. Сырьё уровня 0 — твёрдое
/// нет: изображения в таком продукте нет вовсе, есть эхо приёмника, и значок
/// над ним обещает то, чего не бывает. Продукт-файл виден насквозь: растр он
/// или не растр, говорит расширение. А о папке без листинга сказать нечего, и
/// «похоже, да» тут честнее отказа — раскладок много, листинг стоит запроса, а
/// не найденный в ней растр объяснит [`nothing_here`].
///
/// `level` — уровень обработки, если он известен (в листинге хранилища его
/// сообщить некому: уровень живёт в атрибутах каталога).
pub fn showable(identifier: &str, folder: bool, level: Option<u32>) -> bool {
    if level == Some(0) {
        return false;
    }
    folder || single(identifier).is_some()
}

/// Форматы, которые открывает наложение, — теми же словами, что и в отказе
/// тайлера: список одному человеку показывают с обеих сторон, и разойтись им
/// незачем.
const READABLE: &str = "PNG, JPEG, TIFF, JPEG 2000, NetCDF, GIF, BMP, WebP";

/// Продукт — один файл, и растром он не является. Что за файл — видно по нему
/// самому, и это весь ответ: распаковывать архивы наложение не умеет.
pub fn just_a_file(identifier: &str) -> String {
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((_, suffix)) => format!(
            "продукт лежит одним файлом .{}, а наложить можно растр — {}",
            suffix.to_ascii_lowercase(),
            READABLE
        ),
        None => format!("продукт лежит одним файлом, а наложить можно растр — {}", READABLE),
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
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    // Сырьё уровня 0 — эхо приёмника, не изображение: собрать снимок из него
    // может только наземный процессор, и растру в таком продукте взяться
    // неоткуда.
    if name.contains("_RAW__0S") {
        return format!(
            "это сырьё уровня 0: изображения в продукте ещё нет, только эхо-сигналы ({} файлов)",
            keys.len()
        );
    }
    format!(
        "среди {} файлов продукта растра узнаваемой раскладки нет — лежат {}",
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
        let rasters = scan(&keys);
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
        let rasters = scan(&keys);
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
        let rasters = scan(&keys);
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
        let rasters = scan(&keys);
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
        let rasters = scan(&clms);
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
        let rasters = scan(&bands);
        assert_eq!(rasters.len(), 2);
        assert!(rasters[0].0.ends_with("_thumb_large.jpg") && rasters[0].1 == Role::Preview);
        assert!(rasters[1].0.ends_with("_B1.TIF") && rasters[1].1 == Role::Detailed);
    }

    /// Цветной снимок целиком идёт вперёд отдельных полос — по имени, потому
    /// что до открытия о файлах больше ничего и не известно.
    #[test]
    fn a_true_colour_file_wins_over_single_bands() {
        let granule = keys(&[
            "eodata/Sentinel-3/SR/x.SEN3/Oa01_radiance.nc",
            "eodata/Sentinel-3/SR/x.SEN3/rgb_TCI.nc",
        ]);
        let rasters = scan(&granule);
        assert_eq!(rasters.len(), 1);
        assert!(rasters[0].0.ends_with("rgb_TCI.nc"));
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
        // Сырьё уровня 0 — изображения в нём нет, сколько ни листай.
        assert!(!showable("eodata/…/S1C_IW_RAW__0SDV.SAFE", true, Some(0)));
        // Архив калибровочных таблиц — один файл, и он не растр.
        assert!(!showable("eodata/…/S2A_OPER_GIP_R2EQOG_B03.TGZ", false, Some(1)));
        // Гранула Sentinel-5P — один файл, и он читается.
        assert!(showable("eodata/…/S5P_NRTI_L2__NO2___.nc", false, Some(2)));
        // Папка: что внутри, скажет листинг.
        assert!(showable("eodata/…/S2C_MSIL2A_T40WFC.SAFE", true, Some(2)));
        // Уровень неизвестен (листинг хранилища) — судим по одному ключу.
        assert!(showable("eodata/…/tile.TIF", false, None));
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
}
