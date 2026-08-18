//! Как устроен каталог CDSE: запрос к OData и разбор ответа.
//!
//! Каталог — служба отдельная от хранилища: адрес другой, ключей не спрашивает
//! (метаданные открыты всем) и отвечает JSON, а не XML. Общее у них одно, зато
//! главное — `S3Path`: путь, которым найденное потом подписывают и качают
//! (см. `s3::key`).

use crate::proto::data_provider::{DataProduct, GeoPoint, Ring, SearchRequest};
use super::scene::{self, Facts};
use url::Url;

const HOST: &str = "catalogue.dataspace.copernicus.eu";
const PATH: &str = "/odata/v1/Products";

/// Сколько снимков отдавать, если заказчик не сказал: экран списка плюс запас
/// на прокрутку.
const DEFAULT_LIMIT: u32 = 50;
/// Больше OData не отдаёт за раз и на запрос сверх этого отвечает отказом.
const MAX_LIMIT: u32 = 1000;

/// Во сколько раз просить продуктов больше, чем снимков нужно.
///
/// Каталог считает продуктами, а мы отвечаем снимками, и одна съёмка Sentinel-1
/// это до пяти продуктов. Без запаса страница выдачи вышла бы вдвое-впятеро
/// короче заказанной. Четыре — потому что дальше растёт уже не выдача, а время
/// ответа: пятьдесят продуктов приезжают за 3 секунды, двести пятьдесят за
/// четыре.
const OVERFETCH: u32 = 4;

/// Сколько снимков просил заказчик. Снимков, а не продуктов: в запрос уходит
/// вчетверо больше (см. [`OVERFETCH`]), и путать эти два числа нельзя — по
/// первому решают, набралась ли страница.
pub fn wanted(request: &SearchRequest) -> u32 {
    match request.limit {
        0 => DEFAULT_LIMIT,
        asked => asked.min(MAX_LIMIT),
    }
}

/// Сколько продуктов уходит в запрос — [`wanted`] с запасом на сведение.
/// Спрашивают это дважды: собирая адрес и разбирая ответ, где по нему видно,
/// кончились ли в окне продукты или мы просто мало попросили.
pub fn asked(request: &SearchRequest) -> u32 {
    wanted(request).saturating_mul(OVERFETCH).min(MAX_LIMIT)
}

/// Адрес запроса поиска.
///
/// Атрибуты просятся всегда: миссия, тип продукта и облачность живут только
/// там, а показать найденное без них — значит показать одни имена файлов.
/// Стоит это около четырёх килобайт на снимок, и выбрать из атрибутов нужные
/// нельзя — вложенный фильтр на `$expand` каталог отвергает.
///
/// `floor` — нижняя граница по времени, которую мы ставим сами, когда заказчик
/// её не задал (unix-секунды; 0 — не ставить). Она не про выдачу, а про
/// скорость: см. [`FRESH_DAYS`].
pub fn search(request: &SearchRequest, floor: i64) -> String {
    // Через Url, а не форматированием: в фильтр попадают кавычки, скобки и
    // пробелы, и кодировать их по месту значит однажды забыть.
    let mut url = Url::parse(&format!("https://{}{}", HOST, PATH))
        .expect("адрес каталога собирается из констант");
    {
        let mut pairs = url.query_pairs_mut();
        let filter = filter(request, floor);
        if !filter.is_empty() {
            pairs.append_pair("$filter", &filter);
        }
        pairs.append_pair("$expand", "Attributes");
        // Свежее сверху: снимок недельной давности нужен чаще, чем снимок
        // десятилетней.
        pairs.append_pair("$orderby", "ContentDate/Start desc");
        pairs.append_pair("$top", &asked(request).to_string());
    }
    url.into()
}

/// Адрес запроса упаковок той же съёмки — второй ход после [`locate`].
///
/// Спрашивается секундой съёмки, а не именем: имена упаковок расходятся
/// хвостом (контрольная сумма упаковки у каждой своя), а время начала у них
/// одно с точностью до долей секунды. Отсюда окно в секунду по обе стороны;
/// лишнее отсеет сборка снимков (`scene::acquisition`).
///
/// Коллекция в фильтре не для отбора, а ради индекса: без неё каталог сортирует
/// весь архив — см. [`FRESH_DAYS`].
pub fn siblings(mission: &str, acquired: i64) -> String {
    let mut url = Url::parse(&format!("https://{}{}", HOST, PATH))
        .expect("адрес каталога собирается из констант");
    {
        let mut pairs = url.query_pairs_mut();
        let mut terms = vec![
            format!("ContentDate/Start ge {}", super::time::format(acquired - 1)),
            format!("ContentDate/Start le {}", super::time::format(acquired + 1)),
        ];
        if !mission.is_empty() {
            terms.push(format!("Collection/Name eq {}", literal(mission)));
        }
        pairs.append_pair("$filter", &terms.join(" and "));
        pairs.append_pair("$expand", "Attributes");
        pairs.append_pair("$top", "30");
    }
    url.into()
}

/// Адрес запроса одного продукта по точному имени — обратный ход поиска: имя
/// корня продукта уже известно из ключа хранилища, нужен сам продукт с
/// футпринтом и атрибутами. Точное равенство, а не `contains`: имя продукта —
/// ключ каталога, и подстрочное совпадение здесь означало бы чужой продукт.
pub fn locate(name: &str) -> String {
    let mut url = Url::parse(&format!("https://{}{}", HOST, PATH))
        .expect("адрес каталога собирается из констант");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("$filter", &format!("Name eq {}", literal(name)));
        pairs.append_pair("$expand", "Attributes");
        pairs.append_pair("$top", "1");
    }
    url.into()
}

/// Сколько суток назад смотрит запрос, у которого нижней границы нет.
///
/// Граница здесь не про выдачу, а про скорость, и она обязательна. Каталог без
/// нижней границы по времени сортирует весь архив, и это не «чуть медленнее»:
/// пустой запрос отвечает за 21 секунду, запрос по области — за минуту, запрос
/// по облачности не отвечает вовсе. С границей те же три — 3, 6 и 1,5 секунды.
///
/// Поставленная нами граница не окончательна: не набралось страницы — запрос
/// повторяется без неё (см. `cdse::on_http_result`), и тогда за медленный ответ
/// уже заплачено делом.
pub const FRESH_DAYS: i64 = 2;

/// Условия запроса через `and`. Пустое поле условия не добавляет, поэтому
/// запрос без единого заполненного поля — это «всё подряд», и от бесконечности
/// его удерживает только `$top`.
fn filter(request: &SearchRequest, floor: i64) -> String {
    let mut terms: Vec<String> = Vec::new();

    if !request.mission.is_empty() {
        terms.push(format!("Collection/Name eq {}", literal(&request.mission)));
    }
    if !request.name.is_empty() {
        terms.push(format!("contains(Name,{})", literal(&request.name)));
    }
    // Своя граница — только когда заказчик не назвал свою.
    if request.from <= 0 && floor > 0 {
        terms.push(format!("ContentDate/Start gt {}", super::time::format(floor)));
    }
    // Меньше трёх точек — не область: такой полигон каталог считает негодным
    // запросом и отвечает отказом на весь поиск.
    if let Some(area) = request.area.as_ref().filter(|area| area.points.len() >= 3) {
        terms.push(format!(
            "OData.CSC.Intersects(area=geography'SRID=4326;{}')",
            polygon(area)
        ));
    }
    // Время — по началу съёмки и без кавычек: литерал даты в OData не строка.
    if request.from > 0 {
        terms.push(format!("ContentDate/Start gt {}", super::time::format(request.from)));
    }
    if request.to > 0 {
        terms.push(format!("ContentDate/Start lt {}", super::time::format(request.to)));
    }
    if let Some(max_cloud) = request.max_cloud {
        terms.push(format!(
            "Attributes/OData.CSC.DoubleAttribute/any(a:a/Name eq 'cloudCover' \
             and a/OData.CSC.DoubleAttribute/Value lt {:.2})",
            max_cloud
        ));
    }

    terms.join(" and ")
}

/// Строка в кавычках. Внутренняя кавычка удваивается — так её экранирует OData,
/// и без этого имя с апострофом обрывало бы выражение фильтра.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Кольцо в WKT. Порядок координат там обратный принятому у нас — сначала
/// долгота, — и замкнутость обязательна.
fn polygon(ring: &Ring) -> String {
    let mut points: Vec<String> = ring
        .points
        .iter()
        // Шести знаков хватает: это около десяти сантиметров на экваторе, а
        // область поиска задают куда грубее.
        .map(|point| format!("{:.6} {:.6}", point.lon, point.lat))
        .collect();
    if points.first() != points.last() {
        points.push(points[0].clone());
    }
    format!("POLYGON(({}))", points.join(","))
}

/// Что́ каталог ответил на поиск.
///
/// Продуктов здесь только число, но обойтись без него нельзя: короткая
/// страница снимков означает две разные вещи. Либо в окне кончились продукты —
/// тогда искать надо шире; либо их пришла полная страница, и снимков вышло
/// меньше только потому, что одна съёмка это несколько продуктов, — и тогда
/// расширять окно незачем, там есть ещё.
pub struct Found {
    pub scenes: Vec<DataProduct>,
    pub products: usize,
}

/// Разбирает ответ каталога в снимки: продукты одной съёмки сводятся в один,
/// вспомогательные данные отсеиваются (см. scene.rs).
pub fn scenes(body: &[u8]) -> anyhow::Result<Found> {
    let products = parse(body)?;
    Ok(Found { products: products.len(), scenes: scene::group(products) })
}

/// Разбирает ответ каталога продуктами, как он их и отдал, — вместе с фактами,
/// по которым из них собираются снимки.
///
/// Врозь со [`scenes`] нужен там, где спрашивают про один известный продукт:
/// сведение снимков отвечает выбранной упаковкой, а «нашёлся ли тот, о ком
/// спросили» по такому ответу уже не сказать.
pub fn parse(body: &[u8]) -> anyhow::Result<Vec<(Facts, DataProduct)>> {
    let response: Response = serde_json::from_slice(body)?;
    Ok(response
        .value
        .into_iter()
        .map(|raw| {
            let facts = facts(&raw);
            let mut product = product(raw);
            // Уровень обработки знает только каталог, а список читаемых
            // форматов — только `imagery`: здесь они и встречаются.
            product.viewable =
                super::imagery::showable(&product.identifier, product.folder, facts.level);
            (facts, product)
        })
        .collect())
}

/// Факты каталога, по которым продукты сводятся в снимки. Живут они в
/// атрибутах, и достаются оттуда здесь — сами правила про них ничего не знают.
fn facts(product: &Product) -> Facts {
    let text = |name: &str| {
        product
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| match &attribute.value {
                // Дататейк и виток каталог отдаёт числами, плитку строкой — все
                // нужны текстом, потому что служат частью ключа.
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    Facts {
        platform: text("platformShortName"),
        instrument: text("instrumentShortName"),
        datatake: text("datatakeID"),
        tile: text("tileId"),
        slice: text("sliceNumber"),
        orbit: text("orbitNumber"),
        second: product.content_date.as_ref().map_or(0, |date| super::time::parse(&date.start)),
        level: scene::level(&text("processingLevel")),
        kind: text("productType"),
        size: product.size,
        framed: product.footprint.is_some(),
    }
}

/// Достаёт из ответа объяснение отказа. У каталога оно лежит в `detail`, и
/// показать его куда полезнее, чем один код состояния: «негодный фильтр» и
/// «нет такой коллекции» с виду одинаковы.
pub fn failure(body: &[u8]) -> Option<String> {
    let response: Response = serde_json::from_slice(body).ok()?;
    let detail = response.detail?;
    let message = match detail.get("message").and_then(|message| message.as_str()) {
        Some(message) => message.to_string(),
        None => detail.to_string(),
    };
    (!message.is_empty()).then_some(message)
}

fn product(product: Product) -> DataProduct {
    let attribute = |name: &str| {
        product
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| &attribute.value)
    };
    let text = |name: &str| {
        attribute(name).and_then(|value| value.as_str()).unwrap_or_default().to_string()
    };

    // Путь каталога начинается со слэша, а идентификатор у нас — без:
    // с ним `s3::key` увидел бы пустое имя бакета.
    let identifier = product.s3_path.trim_start_matches('/').to_string();
    let folder = !super::s3::is_single_object(&identifier);
    DataProduct {
        identifier,
        name: product.name,
        acquired: product.content_date.map(|date| super::time::parse(&date.start)).unwrap_or(0),
        size: product.size,
        footprint: product.footprint.map(rings).unwrap_or_default(),
        mission: text("platformShortName"),
        product_type: text("productType"),
        cloud_cover: attribute("cloudCover").and_then(|value| value.as_f64()),
        online: product.online,
        folder,
        // Части проставляет сведение снимков — оно одно видит соседей, —
        // а `viewable` требует уровня обработки и ставится в [`parse`].
        parts: Vec::new(),
        viewable: false,
    }
}

/// GeoJSON-геометрия в кольца.
///
/// Дырки полигона попадают сюда наравне с внешним контуром: очерчены они так же,
/// а закрашивать здесь нечего — рисуются кольца линиями.
fn rings(geometry: Geometry) -> Vec<Ring> {
    let polygons: Vec<Vec<Vec<[f64; 2]>>> = match geometry.kind.as_str() {
        // Снимок через 180-й меридиан каталог отдаёт разрезанным, и тогда
        // полигонов несколько.
        "MultiPolygon" => serde_json::from_value(geometry.coordinates).unwrap_or_default(),
        "Polygon" => serde_json::from_value(geometry.coordinates)
            .map(|rings| vec![rings])
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    polygons
        .into_iter()
        .flatten()
        .map(|ring| Ring {
            // В GeoJSON координаты идут долготой вперёд.
            points: closed(ring)
                .iter()
                .map(|&[lon, lat]| GeoPoint { lat, lon })
                .collect(),
        })
        .collect()
}

/// Кольцо без замыкающей вершины: в GeoJSON последняя точка повторяет первую,
/// и это запись о замкнутости, а не отдельная вершина. Считающему её за
/// вершину она перекашивает всё, что выводится из набора точек, — середину
/// контура в первую очередь: у прямоугольника, повторившего южный угол, центр
/// уезжает к югу.
fn closed(ring: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    match ring.as_slice() {
        [first, .., last] if first == last => ring[..ring.len() - 1].to_vec(),
        _ => ring,
    }
}

#[derive(serde::Deserialize)]
struct Response {
    #[serde(default)]
    value: Vec<Product>,
    /// Есть только у отказа.
    #[serde(default)]
    detail: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct Product {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "S3Path", default)]
    s3_path: String,
    #[serde(rename = "ContentLength", default)]
    size: u64,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "ContentDate", default)]
    content_date: Option<ContentDate>,
    #[serde(rename = "GeoFootprint", default)]
    footprint: Option<Geometry>,
    #[serde(rename = "Attributes", default)]
    attributes: Vec<Attribute>,
}

#[derive(serde::Deserialize)]
struct ContentDate {
    #[serde(rename = "Start", default)]
    start: String,
}

#[derive(serde::Deserialize)]
struct Geometry {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    coordinates: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct Attribute {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Value", default)]
    value: serde_json::Value,
}
