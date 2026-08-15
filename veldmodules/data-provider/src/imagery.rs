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

    rasters
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

    #[test]
    fn unknown_product_yields_nothing() {
        assert!(scan(&keys(&["eodata/CLMS/Vegetation/ndvi.nc"])).is_empty());
    }
}
