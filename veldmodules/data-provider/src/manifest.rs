//! Манифест продукта: что в нём измерение, сказано им самим.
//!
//! У всякого `.SAFE` и `.SEN3` в корне лежит манифест XFDU — `manifest.safe`
//! либо `xfdumanifest.xml`. Это не метаданные про съёмку, а опись самого
//! пакета: карта содержимого называет единицы данных с их родом, а раздел
//! объектов — файлы, которыми эти единицы записаны. Единица рода «Measurement
//! Data Unit» и есть измерение; всё прочее — геометрия, координаты, флаги
//! качества, таблицы.
//!
//! ```xml
//! <xfdu:contentUnit ID="NTC_AOD_Unit" unitType="Measurement Data Unit" …>
//!   <dataObjectPointer dataObjectID="NTC_AOD_Data"/>
//! …
//! <dataObject ID="NTC_AOD_Data">
//!   <byteStream mimeType="application/x-netcdf" size="35064511">
//!     <fileLocation locatorType="URL" href="NTC_AOD.nc"/>
//! ```
//!
//! Без манифеста выбирать приходится по именам файлов, и это гадание
//! ошибается там, где ошибиться дороже всего: у гранулы SLSTR
//! `LST_ancillary_ds.nc` стои́т в алфавите раньше `LST_in.nc`, а у Sentinel-1
//! OCN раньше всего стои́т `preview/icons/logo.png`. Манифест снимает вопрос
//! целиком и стоит одного запроса на 20–300 КБ.

use std::collections::{HashMap, HashSet};

use quick_xml::events::Event;
use quick_xml::Reader;

/// Имена манифеста. Их два, потому что упаковок две: `.SAFE` у Sentinel-1 и
/// Sentinel-2, `.SEN3` у Sentinel-3. Внутри — один и тот же XFDU.
const NAMES: [&str; 2] = ["manifest.safe", "xfdumanifest.xml"];

/// Открывающий тег или какой-то другой. Нужно это одному месту — вложенности
/// единиц, — а `Event` о себе такого не сообщает: `Start` и `Empty` несут одну
/// и ту же `BytesStart`.
enum Kind {
    Start,
    Other,
}

/// Манифест в корне продукта — тот единственный ключ, чей последний сегмент
/// один из [`NAMES`], а до него ровно путь продукта.
///
/// Именно в корне: у Sentinel-2 манифесты лежат ещё и в гранулах, и взятый
/// оттуда описывал бы часть продукта вместо целого.
pub fn key<'a>(identifier: &str, keys: &'a [String]) -> Option<&'a String> {
    let root = format!("{}/", identifier.trim_end_matches('/'));
    keys.iter().find(|key| {
        NAMES.iter().any(|name| {
            key.len() == root.len() + name.len() && key.starts_with(&root) && key.ends_with(name)
        })
    })
}

/// Файлы, которыми записано измерение, — путями относительно корня продукта, в
/// порядке манифеста.
///
/// Пусто — манифест не разобрался или ни одной единицы измерения не назвал.
/// Это не ошибка: ответ «не знаю» здесь означает ровно то, что было до
/// манифеста, и разбор возвращается к именам файлов.
pub fn measurements(body: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    // Единицы вложены одна в другую («Information Package» → сама единица), и
    // род объявлен на внешней ровно так же, как на внутренней. Поэтому
    // указатель считается измерением, если род измерения объявлен хоть у одного
    // предка: стек хранит, сколько их сейчас открыто.
    let mut depth: Vec<bool> = Vec::new();
    let mut measured: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut files: HashMap<String, String> = HashMap::new();
    let mut object = String::new();
    let mut buf = Vec::new();

    loop {
        let event = match reader.read_event_into(&mut buf) {
            Ok(event) => event,
            // Оборванный или чужой XML — то же «не знаю»: половина манифеста
            // назвала бы половину измерений, и это хуже, чем не назвать ничего.
            Err(_) => return Vec::new(),
        };
        let event_kind = match event {
            Event::Start(_) => Kind::Start,
            _ => Kind::Other,
        };
        match event {
            // Открывающий и самозакрывающийся тег читаются одинаково, а
            // различает их одно: у самозакрывающегося нет детей, и на стек
            // вложенности он не встаёт.
            Event::Start(tag) | Event::Empty(tag) => {
                let opens = matches!(event_kind, Kind::Start);
                match tag.local_name().as_ref() {
                    b"contentUnit" if opens => {
                        depth.push(is_measurement(&attribute(&tag, b"unitType")))
                    }
                    b"dataObjectPointer" if depth.iter().any(|unit| *unit) => {
                        let id = attribute(&tag, b"dataObjectID");
                        if !id.is_empty() && measured.insert(id.clone()) {
                            order.push(id);
                        }
                    }
                    b"dataObject" if opens => object = attribute(&tag, b"ID"),
                    b"fileLocation" if !object.is_empty() => {
                        files.insert(object.clone(), relative(&attribute(&tag, b"href")));
                    }
                    _ => {}
                }
            }
            Event::End(tag) => match tag.local_name().as_ref() {
                b"contentUnit" => {
                    depth.pop();
                }
                b"dataObject" => object.clear(),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    order.into_iter().filter_map(|id| files.remove(&id)).filter(|href| !href.is_empty()).collect()
}

/// Род единицы, объявленный измерением. Сравнение по слову «measurement»: в
/// XFDU рядом стоя́т «Measurement Data Unit» у Sentinel-3 и тот же род с
/// собственным `repID` у Sentinel-1, а прочие роды — «Annotation Data Unit»,
/// «Metadata Unit», «Information Package» — этого слова не содержат.
fn is_measurement(unit_type: &str) -> bool {
    unit_type.to_ascii_lowercase().contains("measurement")
}

/// Значение атрибута по имени; пусто — атрибута нет.
fn attribute(tag: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> String {
    tag.attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).to_string())
        .unwrap_or_default()
}

/// Путь из `href` в путь от корня продукта: `./measurement/x.tiff` и
/// `measurement/x.tiff` — один и тот же файл, а ведущий слэш в манифесте
/// означает то же самое, а не корень бакета.
fn relative(href: &str) -> String {
    href.trim_start_matches("./").trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Настоящий манифест Sentinel-3 (`S3A_SY_2_AOD…SEN3`), укороченный до
    /// разбираемого: одна единица измерения внутри пакета.
    const SEN3: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xfdu:XFDU xmlns:xfdu="urn:ccsds:schema:xfdu:1" version="esa-1.0">
  <informationPackageMap>
    <xfdu:contentUnit ID="packageUnit" unitType="Information Package" textInfo="SYNERGY AOD Package">
      <xfdu:contentUnit ID="NTC_AOD_Unit" unitType="Measurement Data Unit" textInfo="AOD">
        <dataObjectPointer dataObjectID="NTC_AOD_Data"/>
      </xfdu:contentUnit>
      <xfdu:contentUnit ID="geoUnit" unitType="Annotation Data Unit" textInfo="Geodetic">
        <dataObjectPointer dataObjectID="geodeticData"/>
      </xfdu:contentUnit>
    </xfdu:contentUnit>
  </informationPackageMap>
  <dataObjectSection>
    <dataObject ID="geodeticData">
      <byteStream mimeType="application/x-netcdf" size="900">
        <fileLocation locatorType="URL" href="geodetic_an.nc"/>
      </byteStream>
    </dataObject>
    <dataObject ID="NTC_AOD_Data">
      <byteStream mimeType="application/x-netcdf" size="35064511">
        <fileLocation locatorType="URL" href="NTC_AOD.nc"/>
        <checksum checksumName="MD5">6cfc77db3ed681308a24a95e703a48d6</checksum>
      </byteStream>
    </dataObject>
  </dataObjectSection>
</xfdu:XFDU>"#;

    /// Манифест Sentinel-1: тот же XFDU, но путь ведёт в подкаталог и записан
    /// от текущего места.
    const SAFE: &str = r#"<xfdu:XFDU xmlns:xfdu="urn:ccsds:schema:xfdu:1">
  <informationPackageMap>
    <xfdu:contentUnit unitType="SAFE Archive Information Package">
      <xfdu:contentUnit unitType="Measurement Data Unit" repID="s1Level2MeasurementSchema">
        <dataObjectPointer dataObjectID="measurementData1"/>
      </xfdu:contentUnit>
      <xfdu:contentUnit unitType="Metadata Unit" repID="s1Level2ProductSchema">
        <dataObjectPointer dataObjectID="productAnnotation"/>
      </xfdu:contentUnit>
    </xfdu:contentUnit>
  </informationPackageMap>
  <dataObjectSection>
    <dataObject ID="measurementData1" repID="s1Level2MeasurementSchema">
      <byteStream mimeType="application/octet-stream" size="41769268">
        <fileLocation locatorType="URL" textInfo="Measurement" href="./measurement/s1c-ew-ocn-hh-001.nc"/>
      </byteStream>
    </dataObject>
    <dataObject ID="productAnnotation">
      <byteStream mimeType="text/xml" size="1024">
        <fileLocation locatorType="URL" href="./annotation/s1c-ew-ocn-hh-001.xml"/>
      </byteStream>
    </dataObject>
  </dataObjectSection>
</xfdu:XFDU>"#;

    /// Названо ровно измерение — и ни геометрия, ни аннотация, хотя лежат они
    /// в том же разделе объектов и в алфавите стоя́т раньше.
    #[test]
    fn the_manifest_names_the_measurement_and_nothing_else() {
        assert_eq!(measurements(SEN3.as_bytes()), vec!["NTC_AOD.nc".to_string()]);
        assert_eq!(
            measurements(SAFE.as_bytes()),
            vec!["measurement/s1c-ew-ocn-hh-001.nc".to_string()]
        );
    }

    /// Не разобралось — значит «не знаю», а не «измерений нет»: половина
    /// ответа хуже отсутствия ответа, потому что на неё положились бы.
    #[test]
    fn a_broken_manifest_says_nothing() {
        assert!(measurements(b"").is_empty());
        assert!(measurements(b"<xfdu:XFDU><informationPackageMap>").is_empty());
        assert!(measurements(b"{\"json\": true}").is_empty());
        // Единица есть, а объекта, на который она указывает, нет.
        let dangling = SEN3.replace("NTC_AOD_Data\">", "otherData\">");
        assert!(measurements(dangling.as_bytes()).is_empty());
    }

    /// Манифест ищется в корне продукта, а не где придётся: у Sentinel-2 такой
    /// же файл лежит в каждой грануле и описывает её одну.
    #[test]
    fn only_the_manifest_at_the_root_counts() {
        let keys: Vec<String> = [
            "eodata/…/S2C_MSIL2A.SAFE/GRANULE/L2A_T40WFC/MTD_TL.xml",
            "eodata/…/S2C_MSIL2A.SAFE/GRANULE/L2A_T40WFC/manifest.safe",
            "eodata/…/S2C_MSIL2A.SAFE/manifest.safe",
        ]
        .iter()
        .map(|path| path.to_string())
        .collect();
        assert_eq!(
            key("eodata/…/S2C_MSIL2A.SAFE", &keys),
            Some(&"eodata/…/S2C_MSIL2A.SAFE/manifest.safe".to_string())
        );
        assert_eq!(key("eodata/…/S2C_MSIL2A.SAFE/", &keys), key("eodata/…/S2C_MSIL2A.SAFE", &keys));
        assert_eq!(key("eodata/…/X.SEN3", &keys), None);
    }
}
