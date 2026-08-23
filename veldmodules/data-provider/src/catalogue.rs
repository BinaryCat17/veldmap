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

/// Адрес запроса к каталогу: у всех трёх запросов он один и тот же, а разнятся
/// они фильтром, порядком (`order`, `None` — как отдаст каталог) и тем, сколько
/// продуктов брать.
///
/// Собирается через `Url`, а не форматированием: в фильтр попадают кавычки,
/// скобки и пробелы, и кодировать их по месту значит однажды забыть.
///
/// Атрибуты просятся всегда: миссия, тип продукта и облачность живут только
/// там, а показать найденное без них — значит показать одни имена файлов.
/// Стоит это около четырёх килобайт на снимок, и выбрать из атрибутов нужные
/// нельзя — вложенный фильтр на `$expand` каталог отвергает.
fn address(filter: &str, order: Option<&str>, top: u32) -> String {
    let mut url = Url::parse(&format!("https://{}{}", HOST, PATH))
        .expect("адрес каталога собирается из констант");
    {
        let mut pairs = url.query_pairs_mut();
        if !filter.is_empty() {
            pairs.append_pair("$filter", filter);
        }
        pairs.append_pair("$expand", "Attributes");
        if let Some(order) = order {
            pairs.append_pair("$orderby", order);
        }
        pairs.append_pair("$top", &top.to_string());
    }
    url.into()
}

/// Адрес запроса поиска.
///
/// `floor` — нижняя граница по времени, которую мы ставим сами, когда заказчик
/// её не задал (unix-секунды; 0 — не ставить). Она не про выдачу, а про
/// скорость: см. [`FRESH_DAYS`].
pub fn search(request: &SearchRequest, floor: i64) -> String {
    // Свежее сверху: снимок недельной давности нужен чаще, чем снимок
    // десятилетней.
    address(&filter(request, floor), Some("ContentDate/Start desc"), asked(request))
}

/// Адрес запроса упаковок той же съёмки — второй ход после [`locate`].
///
/// Спрашивается временем съёмки, а не именем: имена упаковок расходятся хвостом
/// (контрольная сумма упаковки у каждой своя).
///
/// Ширина окна идёт за правилом сведения (`scene::acquisition`), потому что
/// спросить у́же, чем сводишь, значит не увидеть в ответе того, кого собирался
/// приложить.
///
/// Секунда по обе стороны — там, где дататейка нет. У всех таких правил секунда
/// съёмки стои́т внутри самого ключа, то есть части одной съёмки заведомо
/// начинаются в одно и то же мгновение; хватило бы и точного равенства, да
/// спросить им нечем — у продуктов время идёт с долями секунды, а спрашивать мы
/// умеем только целыми (см. `time::format`).
///
/// Десять секунд — там, где дататейк есть. Такие части нарезаны порознь и
/// начинаются в разное время: сырьё на четыре секунды раньше обработанного,
/// комплексное на полторы, — а правило «по слайсу» времени не ограничивает
/// вовсе. Своей границы у него нет, и берётся она у соседнего правила
/// (`scene::BESIDE_A_SLICE_S`), которое той же десяткой прикладывает к слайсу
/// части без номера: измеренный разброс она накрывает с запасом и до соседнего
/// слайса, стоящего в двадцати пяти секундах, не дотягивается.
///
/// Дататейк, а не номер слайса: номера каталог называет не всем частям
/// (`IW_OCN__2S` его не имеет вовсе), а прикладываются они всё тем же окном.
/// Спрашивай мы по номеру, одна и та же съёмка раскрывалась бы по-разному в
/// зависимости от того, про какую её часть спросили.
///
/// Лишнее из ответа отсеет сравнение ключей съёмки — оно в
/// `cdse::on_http_result`, и оно точное (`scene::acquisition`).
///
/// Коллекция в фильтре не для отбора, а ради индекса: без неё каталог сортирует
/// весь архив — см. [`FRESH_DAYS`].
pub fn siblings(mission: &str, tile: &str, datatake: &str, acquired: i64) -> String {
    let window = match datatake.is_empty() {
        true => 1,
        false => scene::BESIDE_A_SLICE_S,
    };
    let mut terms = vec![
        format!("ContentDate/Start ge {}", super::time::format(acquired - window)),
        format!("ContentDate/Start le {}", super::time::format(acquired + window)),
    ];
    if !mission.is_empty() {
        terms.push(format!("Collection/Name eq {}", literal(mission)));
    }
    // Плитка, если каталог её назвал. Секундное окно у Sentinel-2 накрывает не
    // упаковки одной плитки, а всю строку съёмки — под три сотни продуктов
    // (проверено по живому каталогу), — и упаковки нужной терялись бы за
    // потолком выдачи по жребию. Плитка сужает то же окно до единиц.
    if !tile.is_empty() {
        terms.push(format!(
            "Attributes/OData.CSC.StringAttribute/any(a:a/Name eq 'tileId'              and a/OData.CSC.StringAttribute/Value eq {})",
            literal(tile)
        ));
    }
    address(&terms.join(" and "), None, siblings_top(datatake))
}

/// Потолок выдачи у запроса соседей.
///
/// Идёт за окном и в той же мере: у съёмки с дататейком окно вдесятеро шире,
/// вдесятеро выше и потолок, — иначе запас плотности, с которым жил секундный
/// запрос, срезался бы вместе с расширением. Упереться в потолок здесь значит
/// потерять нужную часть по жребию: порядка выдачи мы не задаём.
///
/// Нижняя мера — не «частей у съёмки единицы»: их бывают десятки. У Sentinel-5P
/// в одну секунду ложится десяток газов, у клеток одного пролёта Sentinel-1 RTC
/// секунда общая на весь пролёт.
///
/// Отдельной величиной затем, что упор в потолок надо заметить, а заметить его
/// может только тот, кто знает, где потолок (`cdse::on_http_result`).
pub fn siblings_top(datatake: &str) -> u32 {
    match datatake.is_empty() {
        true => 30,
        false => 300,
    }
}

/// Адрес запроса одного продукта по точному имени — обратный ход поиска: имя
/// корня продукта уже известно из ключа хранилища, нужен сам продукт с
/// футпринтом и атрибутами. Точное равенство, а не `contains`: имя продукта —
/// ключ каталога, и подстрочное совпадение здесь означало бы чужой продукт.
pub fn locate(name: &str) -> String {
    address(&format!("Name eq {}", literal(name)), None, 1)
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
    // Своя граница — только когда заказчик не назвал ни одной своей. Верхняя
    // считается наравне с нижней: «всё до 2020 года» с приписанным «свежее
    // позавчерашнего» — пустое пересечение по построению, и первый заход уходит
    // в сеть заведомо ни за чем.
    if request.from <= 0 && request.to <= 0 && floor > 0 {
        terms.push(format!("ContentDate/Start gt {}", super::time::format(floor)));
    }
    // Меньше трёх точек — не область: такой полигон каталог считает негодным
    // запросом и отвечает отказом на весь поиск. Молчать об этом нельзя:
    // отброшенное условие означает не «чуть шире», а «вся Земля», и выдача
    // после него выглядит правдоподобно — прочие условия-то остались.
    match request.area.as_ref() {
        Some(area) if area.points.len() >= 3 => terms.push(format!(
            "OData.CSC.Intersects(area=geography'SRID=4326;{}')",
            polygon(area)
        )),
        Some(area) => log::warn!(target: "handlers",
            "область из {} вершин — не область, поиск идёт по всей Земле", area.points.len()),
        None => {}
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
    let mut lost = 0usize;
    let parsed: Vec<(Facts, DataProduct)> = response
        .value
        .into_iter()
        .map(|raw| {
            let mut facts = facts(&raw);
            let had_geometry = raw.footprint.is_some();
            let mut product = product(raw);
            // Уровень обработки знает только каталог, а список читаемых
            // форматов — только `imagery`: здесь они и встречаются.
            product.unviewable =
                super::imagery::unviewable(&product.identifier, product.folder, facts.level);
            // «Снимок это или вспомогательные данные» решает контур, и контур
            // тут один — тот, что вышел кольцами. Присутствие поля в ответе
            // каталога тем же самым не является, и цена ошибки здесь не
            // «снимок без контура», а «снимка нет»: бесконтурное сведение
            // снимков считает вспомогательными данными и выбрасывает
            // (`scene::group`). Потому и считается вслух — см. `lost` ниже.
            facts.framed = !product.footprint.is_empty();
            lost += usize::from(had_geometry && product.footprint.is_empty());
            (facts, product)
        })
        .collect();

    // Одной строкой на ответ, а не на продукт: расходится обычно формат, то
    // есть сразу все, и тысяча одинаковых строк сказала бы то же самое.
    if lost > 0 {
        log::warn!(target: "catalogue",
            "геометрия не разобрана у {} продуктов из {}: они уйдут из выдачи как \
             вспомогательные данные", lost, parsed.len());
    }
    Ok(parsed)
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
        // Каталог сообщает дататейк не всему архиву; чего он не сказал,
        // сказано в самом имени (см. `scene::datatake_in_name`).
        datatake: match text("datatakeID") {
            said if !said.is_empty() => said,
            _ => scene::datatake_in_name(&product.name).unwrap_or_default(),
        },
        // Плитку каталог называет не всем: у клеток Sentinel-1 RTC `tileId`
        // пуст, а сама клетка стои́т в начале имени. Она едет отдельным полем —
        // съёмку называет, а в запрос к каталогу не идёт (см. `Facts`).
        tile: text("tileId"),
        named_tile: scene::tile_in_name(&product.name).unwrap_or_default(),
        slice: text("sliceNumber"),
        orbit: text("orbitNumber"),
        second: product.content_date.as_ref().map_or(0, |date| super::time::parse(&date.start)),
        level: scene::level(&text("processingLevel")),
        kind: text("productType"),
        size: product.size,
        // Ставится в [`parse`]: контур — это разобранные кольца, а не поле в
        // ответе каталога.
        framed: false,
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
        product_type: text("productType"),
        cloud_cover: attribute("cloudCover").and_then(|value| value.as_f64()),
        folder,
        // Части проставляет сведение снимков — оно одно видит соседей, —
        // а причина отказа требует уровня обработки и ставится в [`parse`].
        parts: Vec::new(),
        unviewable: String::new(),
    }
}

/// GeoJSON-геометрия в кольца.
///
/// Дырки полигона попадают сюда наравне с внешним контуром: очерчены они так же,
/// а закрашивать здесь нечего — рисуются кольца линиями.
///
/// Ломаные — тоже кольца, и по той же причине: рисуются они линиями, а
/// замкнутость для этого не нужна. В сегодняшнем архиве CDSE их не отдаёт
/// никто — у надирного альтиметра контур и тот полигон, — но GeoJSON такую
/// геометрию разрешает, а отброшенная, она оставила бы снимок без единой
/// вершины при `framed`, говорящем, что контур есть (см. [`parse`]).
fn rings(geometry: Geometry) -> Vec<Ring> {
    // Наружу все формы приходят одним видом — списком ломаных: полигон это
    // замкнутая ломаная, а разница между «через шов разрезано на два» и «две
    // ломаные» рисующему не важна.
    let lines: Vec<Vec<Vertex>> = match geometry.kind.as_str() {
        // Снимок через 180-й меридиан каталог отдаёт разрезанным, и тогда
        // полигонов несколько.
        "MultiPolygon" => serde_json::from_value::<Vec<Vec<Vec<Vertex>>>>(geometry.coordinates)
            .map(|polygons| polygons.into_iter().flatten().collect())
            .unwrap_or_default(),
        "Polygon" | "MultiLineString" => {
            serde_json::from_value(geometry.coordinates).unwrap_or_default()
        }
        "LineString" => serde_json::from_value(geometry.coordinates)
            .map(|line| vec![line])
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    lines
        .into_iter()
        .filter_map(|line| {
            let points: Vec<GeoPoint> = line.iter().filter_map(place).collect();
            // Кольцо уходит целиком либо не уходит вовсе. Выброшенная поодиночке
            // негодная вершина меняет фигуру молча: у снимка о четырёх углах
            // остаётся три, и на шар он ложится не квадом привязки, а грубым
            // габаритом. Отброшенное целиком, кольцо попадает в счёт
            // неразобранной геометрии и говорит о себе строкой в логе
            // (см. [`parse`]).
            (points.len() == line.len()).then(|| Ring { points: closed(points) })
        })
        .collect()
}

/// Вершина GeoJSON. Списком, а не парой: RFC 7946 разрешает третью координату,
/// и пара отвергла бы такую вершину разбором — а вместе с ней и всю геометрию
/// продукта, потому что разбор кончается `unwrap_or_default`. Снимок ушёл бы из
/// выдачи вспомогательными данными (см. [`parse`]) из-за числа, которое здесь
/// и не нужно.
type Vertex = Vec<f64>;

/// Место вершины. В GeoJSON координаты идут долготой вперёд, а высота, если её
/// записали, отбрасывается: контур лежит на поверхности, и высоты у него нет
/// нигде дальше по течению.
///
/// Вершина короче пары места не называет, и молча подставленный ноль увёл бы
/// контур в Гвинейский залив. Что делать с таким кольцом, решает [`rings`].
fn place(vertex: &Vertex) -> Option<GeoPoint> {
    match vertex.as_slice() {
        [lon, lat, ..] => Some(GeoPoint { lat: *lat, lon: *lon }),
        _ => None,
    }
}

/// Кольцо без замыкающей вершины: в GeoJSON последняя точка повторяет первую,
/// и это запись о замкнутости, а не отдельная вершина. Считающему её за
/// вершину она перекашивает всё, что выводится из набора точек, — середину
/// контура в первую очередь: у прямоугольника, повторившего южный угол, центр
/// уезжает к югу.
///
/// Сравниваются места, а не записи вершин: замкнутость — это про то, где
/// вершина лежит. Кольцо, у которого концы записаны с разной высотой, замкнуто
/// не меньше остальных, а сравнение записей объявило бы его разомкнутым, и
/// лишняя вершина поехала бы дальше.
fn closed(mut ring: Vec<GeoPoint>) -> Vec<GeoPoint> {
    if let [first, .., last] = ring.as_slice() {
        if first == last {
            ring.pop();
        }
    }
    ring
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

#[cfg(test)]
mod tests {
    use super::super::time;
    use super::*;

    /// Разбор ответа проставляет причину отказа каждому продукту.
    ///
    /// Уровень обработки знает только каталог, а список читаемых форматов —
    /// только `imagery`, и встречаются они ровно здесь. Пропущенная встреча
    /// компилятору незаметна, а стои́т дорого: продукт уедет с пустой причиной,
    /// то есть с обещанием положить на шар сырьё уровня 0, в котором
    /// изображения нет вовсе.
    #[test]
    fn parsing_tells_every_product_why_the_globe_is_out() {
        let body = br#"{"value":[
            {"Name":"S1C_IW_RAW__0SDV_20260101.SAFE",
             "S3Path":"/eodata/Sentinel-1/S1C_IW_RAW__0SDV_20260101.SAFE",
             "Attributes":[{"Name":"processingLevel","Value":"LEVEL0"}]},
            {"Name":"S2A_OPER_GIP_R2EQOG_B03.TGZ",
             "S3Path":"/eodata/AUX/S2A_OPER_GIP_R2EQOG_B03.TGZ",
             "Attributes":[{"Name":"processingLevel","Value":"LEVEL1C"}]},
            {"Name":"S2C_MSIL2A_T40WFC.SAFE",
             "S3Path":"/eodata/Sentinel-2/S2C_MSIL2A_T40WFC.SAFE",
             "Attributes":[{"Name":"processingLevel","Value":"LEVEL2A"}]}
        ]}"#;
        let parsed = parse(body).expect("ответ каталога разобран");
        let said: Vec<&str> = parsed.iter().map(|(_, p)| p.unviewable.as_str()).collect();
        assert!(said[0].contains("уровня 0"), "сырьё уехало без причины: {:?}", said[0]);
        assert!(said[1].contains(".tgz"), "архив уехал без причины: {:?}", said[1]);
        assert!(said[2].is_empty(), "снимку отказали: {:?}", said[2]);
    }

    /// Значение одного параметра запроса — уже раскодированное, каким его
    /// прочтёт каталог.
    fn param(url: &str, name: &str) -> Option<String> {
        Url::parse(url)
            .expect("собранный адрес разбирается обратно")
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    /// Постоянная часть есть у всех трёх запросов, и обе её половины
    /// обязательны: без атрибутов ответ каталога — одни имена файлов, без
    /// `$top` — весь архив. Порядок просит только поиск: у запросов за одним
    /// известным продуктом сортировать нечего.
    #[test]
    fn every_request_carries_attributes_and_a_limit() {
        let requests = [
            search(&SearchRequest { limit: 10, ..Default::default() }, 0),
            siblings("SENTINEL-1", "", "", 0),
            locate("S2B_MSIL1C"),
        ];
        for url in &requests {
            assert_eq!(param(url, "$expand").as_deref(), Some("Attributes"), "{}", url);
            assert!(param(url, "$top").is_some(), "{}", url);
        }
        // Снимков просили десять, продуктов уходит вчетверо больше — см. [`OVERFETCH`].
        assert_eq!(param(&requests[0], "$top").as_deref(), Some("40"));
        assert!(param(&requests[0], "$orderby").is_some());
        assert!(param(&requests[1], "$orderby").is_none());
        assert!(param(&requests[2], "$orderby").is_none());
    }

    /// Снятое надирным прибором — линия, а не площадь, и каталог отдаёт её
    /// ломаной. Отброшенная, она оставляла снимок в списке без единой вершины.
    #[test]
    fn a_track_is_an_outline_too() {
        let line = |kind: &str, coordinates: serde_json::Value| Geometry {
            kind: kind.to_string(),
            coordinates,
        };
        let track = rings(line(
            "MultiLineString",
            serde_json::json!([[[166.3, -81.4], [165.0, -80.0]], [[10.0, 0.0], [11.0, 1.0]]]),
        ));
        assert_eq!(track.len(), 2, "{:?}", track);
        assert_eq!(track[0].points.len(), 2);
        assert_eq!(track[0].points[0].lat, -81.4);
        assert_eq!(track[0].points[0].lon, 166.3);

        let one = rings(line("LineString", serde_json::json!([[1.0, 2.0], [3.0, 4.0]])));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].points.len(), 2);

        // Полигон и разрезанный швом полигон остаются тем, чем были.
        let square = serde_json::json!([[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]);
        assert_eq!(rings(line("Polygon", square.clone()))[0].points.len(), 3);
        assert_eq!(rings(line("MultiPolygon", serde_json::json!([square]))).len(), 1);

        // Точка контуром не становится: рисовать по ней нечего.
        assert!(rings(line("Point", serde_json::json!([1.0, 2.0]))).is_empty());
    }

    /// Третья координата вершины законна по RFC 7946, и кольцо с ней остаётся
    /// кольцом: отвергнутая разбором, она унесла бы с собой всю геометрию
    /// продукта, а с ней и сам продукт — бесконтурное сведение считает
    /// вспомогательными данными.
    #[test]
    fn a_vertex_may_carry_a_height() {
        let raised = Geometry {
            kind: "Polygon".to_string(),
            coordinates: serde_json::json!([[
                [0.0, 0.0, 137.0],
                [1.0, 0.0, 140.0],
                [1.0, 1.0, 152.0],
                [0.0, 0.0, 137.0]
            ]]),
        };
        let ring = rings(raised);
        assert_eq!(ring.len(), 1, "кольцо потерялось вместе с высотой");
        assert_eq!(ring[0].points.len(), 3, "замыкающая вершина не отброшена");
        assert_eq!((ring[0].points[0].lat, ring[0].points[0].lon), (0.0, 0.0));
        // Несимметричная вершина: у симметричных перестановку широты с
        // долготой не увидеть.
        assert_eq!((ring[0].points[1].lat, ring[0].points[1].lon), (0.0, 1.0));
        assert_eq!((ring[0].points[2].lat, ring[0].points[2].lon), (1.0, 1.0));
    }

    /// А вершина короче пары уносит с собой всё кольцо: место по ней не
    /// назвать, а выброшенная поодиночке она молча подменила бы фигуру. Ушедшее
    /// целиком кольцо каталог посчитает неразобранной геометрией и скажет об
    /// этом вслух (см. [`parse`]).
    #[test]
    fn a_vertex_shorter_than_a_pair_takes_the_ring_with_it() {
        let broken = Geometry {
            kind: "LineString".to_string(),
            coordinates: serde_json::json!([[10.0], [11.0, 1.0]]),
        };
        assert!(rings(broken).is_empty());
    }

    /// Замкнутость — про место вершины, а не про её запись: кольцо, у которого
    /// концы названы с разной высотой, замкнуто, и замыкающая вершина у него
    /// отбрасывается наравне с остальными.
    #[test]
    fn a_ring_closes_by_place_and_not_by_record() {
        let mixed = Geometry {
            kind: "Polygon".to_string(),
            coordinates: serde_json::json!([[
                [0.0, 0.0],
                [1.0, 0.0, 140.0],
                [1.0, 1.0],
                [0.0, 0.0, 137.0]
            ]]),
        };
        let ring = rings(mixed);
        assert_eq!(ring[0].points.len(), 3, "замыкание не распознано");
    }

    /// Запрос без единого условия — «всё подряд»: пустой фильтр не уходит
    /// вовсе, потому что пустое выражение каталог считает негодным.
    #[test]
    fn a_request_without_conditions_has_no_filter() {
        assert_eq!(param(&search(&SearchRequest::default(), 0), "$filter"), None);
    }

    /// Окно упаковок одной плитки — секунда по обе стороны от съёмки:
    /// начинаются они разом, спрашивать точным временем нечем, а целые секунды
    /// с долями у продуктов не совпадут.
    #[test]
    fn siblings_ask_a_window_of_one_second() {
        let acquired = time::parse("2024-05-04T08:23:58.000Z");
        assert_eq!(
            param(&siblings("SENTINEL-1", "", "", acquired), "$filter").as_deref(),
            Some(
                "ContentDate/Start ge 2024-05-04T08:23:57.000Z and \
                 ContentDate/Start le 2024-05-04T08:23:59.000Z and \
                 Collection/Name eq 'SENTINEL-1'"
            )
        );
    }

    /// Нарезанную съёмку спрашивают тем же окном, каким её части сводятся в
    /// снимок, и не шире.
    ///
    /// Разойдись эти двое — и часть, которой снимок обязан показаться, не
    /// попала бы в ответ каталога вовсе: прикладывать было бы нечего, а отказа
    /// никто бы не увидел. Верхний край держится не для красоты: дотянись окно
    /// до соседнего слайса, и его части съели бы потолок выдачи, из которого
    /// нужная вылетела бы по жребию.
    #[test]
    fn siblings_ask_as_wide_as_the_scene_is_gathered() {
        // Соседние слайсы стоя́т друг от друга на двадцать пять секунд и
        // больше — см. `scene::BESIDE_A_SLICE_S`.
        const NEXT_SLICE_S: i64 = 25;
        let acquired = time::parse("2024-05-04T08:23:58.000Z");
        let filter = param(&siblings("SENTINEL-1", "", "191698", acquired), "$filter")
            .expect("фильтр собран");

        let bound = |edge: &str| {
            let at = filter.find(edge).expect("граница окна") + edge.len();
            time::parse(&filter[at..at + "2024-05-04T08:23:58.000Z".len()])
        };
        // Сырьё начинается на четыре секунды раньше обработанного — то же
        // число, на котором стои́т `scene::one_slice_is_one_scene_however_its_parts_start`.
        // Названо здесь отдельно затем, что мерка сведения обязана накрывать
        // не только себя, но и этот, измеренный по живому каталогу, край.
        let spread = scene::BESIDE_A_SLICE_S.max(4);
        for (edge, low, high) in [
            ("ContentDate/Start ge ", acquired - NEXT_SLICE_S + 1, acquired - spread),
            ("ContentDate/Start le ", acquired + spread, acquired + NEXT_SLICE_S - 1),
        ] {
            let at = bound(edge);
            assert!(
                at >= low && at <= high,
                "граница «{}» вышла из [{}, {}]: {}",
                edge.trim_end(),
                time::format(low),
                time::format(high),
                filter
            );
        }
    }

    /// Потолок выдачи идёт за окном. Отстань он — и расширенное окно набрало бы
    /// в ответ соседей плотнее, чем потолок пускает, а срезает он случайных:
    /// порядка у этого запроса нет.
    #[test]
    fn the_siblings_ceiling_follows_the_window() {
        let acquired = time::parse("2024-05-04T08:23:58.000Z");
        let narrow = param(&siblings("SENTINEL-2", "", "", acquired), "$top")
            .expect("потолок назван");
        let wide = param(&siblings("SENTINEL-1", "", "191698", acquired), "$top")
            .expect("потолок назван");
        let narrow = narrow.parse::<i64>().expect("число");
        assert_eq!(narrow, siblings_top("") as i64);
        // Десятки, а не единицы: у Sentinel-5P в одну секунду ложится десяток
        // газов, у клеток одного пролёта RTC секунда общая на весь пролёт.
        assert!(narrow >= 30, "узкому окну потолка не хватит и на одну съёмку: {}", narrow);
        assert!(
            wide.parse::<i64>().expect("число") >= narrow * scene::BESIDE_A_SLICE_S,
            "окно шире вдесятеро, а потолок — нет: {} против {}",
            wide,
            narrow
        );
    }

    /// Плитка сужает то же окно: у Sentinel-2 секунда по обе стороны накрывает
    /// не упаковки одной плитки, а всю строку съёмки, и нужная терялась бы за
    /// потолком выдачи.
    #[test]
    fn siblings_narrow_down_to_the_tile_when_it_is_known() {
        let acquired = time::parse("2026-08-18T01:21:12.000Z");
        let filter = param(&siblings("SENTINEL-2", "55SBD", "", acquired), "$filter")
            .expect("фильтр собран");
        assert!(filter.contains("Collection/Name eq 'SENTINEL-2'"), "{}", filter);
        assert!(filter.contains("a/Name eq 'tileId'"), "{}", filter);
        assert!(filter.contains("a/OData.CSC.StringAttribute/Value eq '55SBD'"), "{}", filter);
        // Плитки нет — нет и терма: у радара её не бывает вовсе.
        let without = param(&siblings("SENTINEL-1", "", "", acquired), "$filter").expect("фильтр");
        assert!(!without.contains("tileId"), "{}", without);
    }

    /// Кавычка в имени доезжает до каталога удвоенной, а не обрывает выражение
    /// фильтра: экранирование OData считается до кодирования адреса.
    #[test]
    fn a_quote_in_the_name_survives_the_url() {
        assert_eq!(
            param(&locate("S2B_D'ART"), "$filter").as_deref(),
            Some("Name eq 'S2B_D''ART'")
        );
    }
}
