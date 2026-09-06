//! Протокол бакета CDSE: адресация, подпись, разбор листинга.
//!
//! Здесь всё, что знает о том, как устроена та сторона, — и ничего о шине.
//! Обработчики (cdse.rs) получают готовый запрос и разобранный ответ.
//!
//! Главное правило модуля — адрес и заголовки не расходятся. SigV4 подписывает
//! конкретный путь, поэтому собирать URL в одном месте, а подпись в другом
//! нельзя: разойдясь, они дают 403 (подпись не о том пути) или 404 (адрес не
//! того объекта). Отсюда `Request` — пара, и создаётся она только здесь.

use std::collections::HashMap;
use std::time::SystemTime;

use aws_sigv4::http_request::{
    sign, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings, UriPathNormalizationMode,
};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use url::Url;

const HOST: &str = "eodata.dataspace.copernicus.eu";
const REGION: &str = "default";
/// Бакет — первый сегмент пути: у CDSE это S3 в path-style адресации.
const BUCKET: &str = "eodata";
/// sha256 пустого тела: у нас все запросы GET, тела нет ни у одного. Подпись в
/// заголовках у S3 обязана нести этот заголовок.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Сколько объектов просить за раз. Листинг постраничный, продолжение — через
/// continuation-token (см. `Listing::next_token`): страница здесь — размер
/// одного ответа хранилища, а не то, что видит пользователь.
const PAGE_SIZE: &str = "200";

/// Подписанный запрос: адрес и заголовки к нему.
pub struct Request {
    pub url: String,
    pub headers: HashMap<String, String>,
}

/// Разобранный листинг одного уровня.
pub struct Listing {
    /// Папки (с завершающим `/`) и объекты вперемешку, в том виде, в каком их
    /// принимают функции этого модуля.
    pub entries: Vec<Entry>,
    /// Пусто — страница последняя.
    pub next_token: String,
}

/// Элемент листинга: ключ и то, что каталог о нём сообщил.
pub struct Entry {
    pub identifier: String,
    pub size: u64,
    pub modified: i64,
}

/// Ключ объекта в бакете из идентификатора продукта.
///
/// Идентификатор — это путь вместе с именем бакета (`eodata/Sentinel-2/…`):
/// в таком виде он показывается в UI, хранится в библиотеке и годится для
/// навигации. Ключ живёт *под* бакетом, поэтому в запрос идёт без префикса,
/// а бакет приписывается адресом.
///
/// Переход между двумя видами — только здесь и в [`identifier`], не по месту
/// в обработчиках: забытый срез префикса даёт `/eodata/eodata/…`, то есть 404
/// на просмотр по сети при исправном скачивании того же файла.
fn key(identifier: &str) -> &str {
    let path = identifier.trim_start_matches('/');
    match path.strip_prefix(BUCKET) {
        Some("") => "",
        // Именно сегмент пути, а не начало имени: у объекта с ключом
        // «eodata-backup/…» префикс срезать нечего.
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => path,
    }
}

/// Обратное преобразование: ключ из ответа S3 → идентификатор для остальных.
fn identifier(key: &str) -> String {
    format!("{}/{}", BUCKET, key)
}

/// Продукт лежит одним объектом, а не каталогом. Хранилище кладёт продукты
/// двумя способами: развёрнутым каталогом (.SAFE, .SEN3, гранулы без
/// суффикса) или одним архивом/файлом (вспомогательные данные, климатика), и
/// каталог CDSE этого различия не сообщает — оно видно только по имени
/// контейнерного формата. Ошибка на одиночном формате вне списка безопасна:
/// «перейти» в такой продукт показывает пустую папку и «вверх», а не 404.
pub fn is_single_object(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    // Растр — тоже один объект, и список его расширений живёт у наложения:
    // разойдись эти два перечисления, и продукт-файл `…/tile.TIF` считался бы
    // папкой, листался бы префиксом и отвечал «в хранилище нет ни одного
    // файла» — при том что значок показа над ним уже нарисован.
    if super::imagery::is_raster(name) {
        return true;
    }
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "tgz" | "zip" | "tar" | "gz" | "n1" | "e1" | "e2" | "hdf" | "dbl"
    )
}

/// Корень продукта, внутри которого лежит ключ. `None` — ключ лежит в пути к
/// снимкам, а не в снимке: ни контейнерного суффикса, ни даты над ним нет.
///
/// Граница продукта проводится одним правилом на два случая, потому что случай
/// один: где кончается путь и начинается снимок.
///  * контейнерный суффикс каталога — .SAFE у Sentinel-1/2, .SEN3 у
///    Sentinel-3: самый мелкий (первый слева) такой сегмент и есть корень;
///  * без суффикса — первый сегмент сразу под тройкой даты съёмки
///    (`…/ГГГГ/ММ/ДД/<снимок>/…`): так лежат Landsat и RTC.
///
/// Отсутствие ответа именно `None`, а не «сам ключ»: «сам себе корень» и
/// «здесь снимка нет» — разные вещи, и слитые в одно они делают папку года
/// снимком. Кому нужен запасной вариант, тот называет его у себя.
pub fn product_root(identifier: &str) -> Option<&str> {
    let trimmed = identifier.trim_end_matches('/');
    let mut offset = 0;
    // Сегменты идут слева направо, и вместе с ними — путь до конца каждого:
    // корень возвращается срезом, а не сборкой строки.
    for segment in trimmed.split('/') {
        let end = offset + segment.len();
        let suffix = segment.rsplit_once('.').map(|(_, suffix)| suffix.to_ascii_lowercase());
        if matches!(suffix.as_deref(), Some("safe" | "sen3")) {
            return Some(&trimmed[..end]);
        }
        // Над этим сегментом стоит дата — значит он и есть снимок.
        if under_date(&trimmed[..offset.saturating_sub(1)]) {
            return Some(&trimmed[..end]);
        }
        offset = end + 1;
    }
    None
}

/// Путь кончается тройкой «год/месяц/день» — тем местом, куда хранилище кладёт
/// сами снимки. Ниже неё папок пути уже нет, поэтому всё, что лежит прямо
/// здесь, — продукт.
fn under_date(path: &str) -> bool {
    let mut tail = path.rsplit('/');
    match (tail.next(), tail.next(), tail.next()) {
        (Some(day), Some(month), Some(year)) => {
            year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && [year, month, day].iter().all(|part| part.bytes().all(|b| b.is_ascii_digit()))
        }
        _ => false,
    }
}

/// GET объекта целиком или диапазоном — Range к подписи не относится и
/// добавляется транспортом (network).
pub fn object(identity: &Identity, identifier: &str) -> Request {
    signed(identity, &format!("/{}/{}", BUCKET, key(identifier)), &[])
}

/// Та же подпись заново — по адресу, а не по идентификатору: просит её
/// network, у которого от объекта остался один адрес. Чужой адрес не
/// подписывается: ключи — к своему хранилищу, а подпись на чужой путь ушла бы
/// туда вместе с ними; без TLS — тоже, по той же причине.
pub fn resign(identity: &Identity, url: &str) -> Option<Request> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(HOST) {
        return None;
    }
    Some(signed(identity, parsed.path(), &[]))
}

/// Листинг одного уровня: папки отдаются как CommonPrefixes, а не разворотом
/// всего поддерева — отсюда delimiter.
pub fn listing(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, true)
}

/// Листинг поддерева целиком: без delimiter S3 разворачивает все ключи под
/// префиксом. Так растры продукта находятся одним запросом на страницу —
/// обходить .SAFE по уровням было бы четыре-пять запросов на гранулу.
pub fn listing_deep(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, false)
}

/// Страница ListObjectsV2. Уровень и поддерево — один и тот же запрос к бакету,
/// и подписывается он одинаково; вся разница между ними — в параметрах.
fn page(identity: &Identity, path: &str, token: &str, by_level: bool) -> Request {
    signed(identity, &format!("/{}/", BUCKET), &query(key(path), token, by_level))
}

/// Параметры страницы листинга.
///
/// Разделитель и есть выбор между «покажи, что лежит прямо здесь» и «разверни
/// всё, что под префиксом», — больше уровень от поддерева ничем не отличается.
/// Пустые prefix и continuation-token не отправляются вовсе: у корня бакета
/// префикса нет, а токен появляется только со второй страницы, и пустым он
/// был бы для хранилища негодным продолжением.
fn query<'a>(prefix: &'a str, token: &'a str, by_level: bool) -> Vec<(&'a str, &'a str)> {
    let mut query = vec![("list-type", "2"), ("max-keys", PAGE_SIZE)];
    if by_level {
        query.push(("delimiter", "/"));
    }
    if !prefix.is_empty() {
        query.push(("prefix", prefix));
    }
    if !token.is_empty() {
        query.push(("continuation-token", token));
    }
    query.sort_by_key(|(name, _)| *name);
    query
}

/// Единственное место, где рождается пара «адрес + подпись».
///
/// Подпись — в заголовках, и живёт она столько, сколько хранилище принимает
/// `x-amz-date`: четверть часа. Подпись в адресе (presign, `X-Amz-*` в строке
/// запроса) хранилище CDSE отвергает `InvalidAccessKeyId` при любом сроке
/// (ADR 0005), так что жизнь открытого ресурса и есть жизнь заголовков.
fn signed(identity: &Identity, path: &str, query: &[(&str, &str)]) -> Request {
    // Через Url, а не форматированием: подписывается ровно та строка запроса,
    // которая уйдёт в сеть, — со всем процентным кодированием.
    let mut url = Url::parse(&format!("https://{}{}", HOST, path))
        .expect("адрес бакета собирается из константы и ключа");
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }

    let signable_path = match url.query() {
        Some(query) => format!("{}?{}", url.path(), query),
        None => url.path().to_string(),
    };

    // Умолчания SigningSettings — не для S3: он канонизирует путь с одинарным
    // процентным кодированием и без нормализации сегментов, а умолчания дают
    // двойное и с нормализацией. На безопасном алфавите ключей Sentinel
    // канонизации совпадают, но ключ с пробелом, «+» или `..` дал бы 403.
    let mut settings = SigningSettings::default();
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;

    let params = v4::SigningParams::builder()
        .identity(identity)
        .region(REGION)
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .expect("параметры подписи заполнены целиком");

    let headers = [("host", HOST), ("x-amz-content-sha256", EMPTY_SHA256)];
    let request = SignableRequest::new(
        "GET",
        &signable_path,
        headers.iter().map(|(name, value)| (*name, *value)),
        SignableBody::Bytes(&[]),
    ).expect("запрос к подписи собран из корректных строк");

    let (instructions, _signature) = sign(request, &params.into())
        .expect("подпись считается локально и не может не получиться")
        .into_parts();

    let mut signed = HashMap::new();
    for (name, value) in instructions.headers() {
        signed.insert(name.to_string(), value.to_string());
    }
    // Заголовок входит в подпись, но инструкции возвращают только вычисленные —
    // тот, что подписывали, надо положить самим.
    signed.insert("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string());

    Request { url: url.to_string(), headers: signed }
}

/// Разбор ответа ListObjectsV2.
///
/// `requested` — путь, который запрашивали: S3 возвращает его сам себе в
/// листинге, и в списке содержимого папке незачем быть своим же элементом.
pub fn parse_listing(body: &[u8], requested: &str) -> anyhow::Result<Listing> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut entries: Vec<Entry> = Vec::new();
    let mut next_token = String::new();
    let mut buf = Vec::new();
    let mut tag = String::new();
    let mut in_common_prefixes = false;
    // Собираемый объект: Key, Size и LastModified приходят разными событиями,
    // и до `</Contents>` элемент неполон.
    let mut object: Option<Entry> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                tag = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match tag.as_str() {
                    "CommonPrefixes" => in_common_prefixes = true,
                    "Contents" => object = Some(Entry { identifier: String::new(), size: 0, modified: 0 }),
                    _ => {}
                }
            }
            Event::End(e) => {
                match e.local_name().as_ref() {
                    b"CommonPrefixes" => in_common_prefixes = false,
                    b"Contents" => {
                        // Ключ со слэшем на конце — не объект, а пустышка,
                        // которой хранилище обозначает папку. Папками в
                        // листинге отвечают общие префиксы, и та же папка
                        // приезжает ими же, а в обходе вглубь папок нет вовсе:
                        // оставленная пустышка стала бы файлом нулевого
                        // размера, который потом «скачивают».
                        match object.take() {
                            Some(entry) if !entry.identifier.ends_with('/') => {
                                push(&mut entries, entry, requested)
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                tag.clear();
            }
            Event::Text(e) => {
                // Раскодировать и раскрыть сущности (&amp;, &#x2F;) — два
                // разных шага: первый снимает кодировку документа, второй
                // работает уже по тексту. Сущности в ключах бакета реальны —
                // слэши и амперсанды в именах продуктов приезжают экранированными.
                let raw = e.xml10_content().unwrap_or_default();
                let text = match quick_xml::escape::unescape(&raw) {
                    Ok(unescaped) => unescaped.into_owned(),
                    Err(_) => raw.into_owned(),
                };
                match (tag.as_str(), &mut object) {
                    // Prefix встречается и вне CommonPrefixes — там это эхо
                    // запроса, а не элемент листинга. У папки нет ни размера,
                    // ни времени: за ней стоит общий префикс ключей, а не
                    // объект.
                    ("Prefix", _) if in_common_prefixes => {
                        push(&mut entries, Entry { identifier: identifier(&text), size: 0, modified: 0 }, requested);
                    }
                    ("Key", Some(entry)) => entry.identifier = identifier(&text),
                    ("Size", Some(entry)) => entry.size = text.parse().unwrap_or(0),
                    ("LastModified", Some(entry)) => entry.modified = super::time::parse(&text),
                    ("NextContinuationToken", _) => next_token = text,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(Listing { entries, next_token })
}

/// Сам запрошенный путь S3 возвращает в листинге своим же элементом — в списке
/// содержимого папке незачем быть собой.
fn push(entries: &mut Vec<Entry>, entry: Entry, requested: &str) {
    if entry.identifier != requested && !entry.identifier.is_empty() {
        entries.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> Identity {
        let credentials = aws_credential_types::Credentials::new("AK", "SK", None, None, "test");
        Identity::new(credentials, None)
    }

    /// Подпись — в заголовках, и у объекта, и у листинга: адрес объекта чист,
    /// заголовок sha256 пустого тела на месте, а подписи в строке запроса нет
    /// — хранилище такую не принимает.
    #[test]
    fn подпись_лежит_в_заголовках_а_не_в_адресе() {
        let identity = test_identity();
        let request = object(&identity, "eodata/Sentinel-2/T31TGK/TCI_10m.jp2");
        let url = Url::parse(&request.url).expect("адрес разбирается");
        assert_eq!(url.path(), "/eodata/Sentinel-2/T31TGK/TCI_10m.jp2");
        assert_eq!(url.query(), None, "у объекта строки запроса нет: {}", request.url);
        assert!(request.headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")), "{:?}", request.headers);
        assert_eq!(request.headers.get("x-amz-content-sha256").map(String::as_str), Some(EMPTY_SHA256));
        assert!(request.headers.keys().any(|name| name.eq_ignore_ascii_case("x-amz-date")));

        let listing = listing(&identity, "eodata/Sentinel-2/", "");
        assert!(listing.headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")), "{:?}", listing.headers);
        assert!(!listing.url.contains("X-Amz-Signature"), "{}", listing.url);
        assert!(listing.url.contains("list-type=2"));
    }

    /// Подпись заново — на тот же адрес и своя; чужой адрес и адрес без TLS
    /// не подписываются.
    #[test]
    fn переподпись_даёт_тот_же_адрес_и_только_своему_хранилищу() {
        let identity = test_identity();
        let request = object(&identity, "eodata/Sentinel-2/T31TGK/TCI_10m.jp2");
        let again = resign(&identity, &request.url).expect("свой адрес подписывается");
        assert_eq!(again.url, request.url);
        assert!(again.headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")), "{:?}", again.headers);
        assert!(resign(&identity, "https://example.com/eodata/x.jp2").is_none(), "чужой адрес");
        assert!(resign(&identity, "http://eodata.dataspace.copernicus.eu/eodata/x.jp2").is_none(), "без TLS ключи не уходят");
    }

    #[test]
    fn product_root_climbs_to_the_container() {
        // Файл в глубине .SAFE — корень поднимается до самого продукта.
        assert_eq!(
            product_root(
                "eodata/Sentinel-2/MSI/L1C/2026/08/12/S2B_MSIL1C_X.SAFE/GRANULE/L1C_T24LVP/IMG_DATA/T24LVP_TCI.jp2"
            ),
            Some("eodata/Sentinel-2/MSI/L1C/2026/08/12/S2B_MSIL1C_X.SAFE")
        );
        // Сам продукт (папкой, со слэшем листинга) — он и есть корень.
        assert_eq!(
            product_root("eodata/Sentinel-1/SAR/IW_GRDH_1S-COG/2026/08/12/S1C_IW_GRDH_1SDV_15A3_COG.SAFE/"),
            Some("eodata/Sentinel-1/SAR/IW_GRDH_1S-COG/2026/08/12/S1C_IW_GRDH_1SDV_15A3_COG.SAFE")
        );
        // Без суффикса корень находится по дате — так лежат Landsat и RTC, и
        // файл внутри такого продукта поднимается до него же.
        assert_eq!(
            product_root("eodata/Sentinel-1-RTC/2024/01/05/S1A_IW_GRDH_RTC/"),
            Some("eodata/Sentinel-1-RTC/2024/01/05/S1A_IW_GRDH_RTC")
        );
        assert_eq!(
            product_root("eodata/Landsat-9/OLI-2/L2SP/2026/08/11/LC09_L2SP_02_T1/LC09_B1.TIF"),
            Some("eodata/Landsat-9/OLI-2/L2SP/2026/08/11/LC09_L2SP_02_T1")
        );
        // Путь к снимкам — ещё не снимок: ни суффикса, ни даты над ним.
        assert_eq!(product_root("eodata/Sentinel-2/MSI/L2A/2026/08"), None);
        assert_eq!(product_root("eodata/CLMS/Vegetation/ndvi_1999_v3.0.1.nc"), None);
    }

    #[test]
    fn single_object_is_told_by_container_suffix() {
        // Одиночные форматы: вспомогательные архивы, климатика.
        assert!(is_single_object(
            "eodata/Sentinel-2/AUX/GIP_R2ABCA/2026/08/13/S2C_OPER_GIP_R2ABCA_MPC_B00.TGZ"
        ));
        assert!(is_single_object("eodata/CLMS/Vegetation/ndvi_1999_v3.0.1.nc"));
        assert!(is_single_object("eodata/Envisat/MER_FRS_1P.N1"));
        // Каталоги: .SAFE и гранулы без контейнерного суффикса.
        assert!(!is_single_object(
            "eodata/Sentinel-1/SAR/IW_GRDH_1S/2026/08/12/S1C_IW_GRDH_1SSV_008959_011C6E_EDC8.SAFE"
        ));
        assert!(!is_single_object("eodata/Sentinel-1-RTC/2024/01/05/S1A_IW_GRDH_RTC"));
        assert!(!is_single_object("eodata/Landsat-5/TM/GTC_1P/1985/02/02/LS05_TM_GTC_1P_4A43"));
    }

    /// Пустышка-«папка» файлом не становится: хранилище кладёт рядом с ключами
    /// объект нулевого размера со слэшем на конце, и в обходе вглубь он —
    /// единственное, что притворяется файлом. Сама папка при этом остаётся в
    /// листинге общим префиксом.
    #[test]
    fn folder_placeholder_is_not_a_file() {
        let body = br#"<?xml version="1.0" encoding="UTF-8"?>
        <ListBucketResult>
          <Contents><Key>Landsat-5/1988/P.TIFF/</Key><Size>0</Size></Contents>
          <Contents><Key>Landsat-5/1988/P.TIFF/B1.TIF</Key><Size>10</Size></Contents>
          <CommonPrefixes><Prefix>Landsat-5/1988/P.TIFF/</Prefix></CommonPrefixes>
        </ListBucketResult>"#;
        let listing = parse_listing(body, "eodata/Landsat-5/1988/").expect("листинг разобран");
        let keys: Vec<&str> = listing.entries.iter().map(|e| e.identifier.as_str()).collect();
        assert_eq!(keys, vec![
            "eodata/Landsat-5/1988/P.TIFF/B1.TIF",
            "eodata/Landsat-5/1988/P.TIFF/",
        ], "папка осталась одна — общим префиксом");
    }

    /// Уровень от поддерева отличает один разделитель, а всё пустое до
    /// хранилища не доезжает: у корня бакета префикса нет, а токен появляется
    /// только со второй страницы.
    #[test]
    fn a_level_differs_from_a_subtree_by_the_delimiter_alone() {
        assert_eq!(
            query("Sentinel-2/MSI/", "", true),
            vec![
                ("delimiter", "/"),
                ("list-type", "2"),
                ("max-keys", PAGE_SIZE),
                ("prefix", "Sentinel-2/MSI/"),
            ]
        );
        assert_eq!(
            query("Sentinel-2/MSI/", "", false),
            vec![("list-type", "2"), ("max-keys", PAGE_SIZE), ("prefix", "Sentinel-2/MSI/")]
        );
        assert_eq!(query("", "", false), vec![("list-type", "2"), ("max-keys", PAGE_SIZE)]);
        assert_eq!(
            query("", "1/N7Ab", false),
            vec![("continuation-token", "1/N7Ab"), ("list-type", "2"), ("max-keys", PAGE_SIZE)]
        );
    }

    /// Дата — это ровно три разряда нужной длины и только из цифр: «08» месяцем
    /// быть может, а «MSI» или «2026-08» — нет.
    #[test]
    fn date_triple_is_matched_by_shape() {
        assert!(under_date("eodata/Sentinel-2/MSI/L2A/2026/08/12"));
        assert!(!under_date("eodata/Sentinel-2/MSI/L2A/2026/08"));
        assert!(!under_date("eodata/Sentinel-2/MSI/L2A/26/08/12"));
        assert!(!under_date("eodata/Sentinel-2/MSI/L2A/2026/8/12"));
        assert!(!under_date("eodata/Sentinel-2/MSI/L2A/2026/AU/12"));
    }
}
