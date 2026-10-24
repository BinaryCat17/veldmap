use crate::proto::data_provider::{DataProduct, GeoPoint, Ring, SearchRequest};
use super::scene::{self, Facts};
use url::Url;

const HOST: &str = "catalogue.dataspace.copernicus.eu";
const PATH: &str = "/odata/v1/Products";

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 1000;

const OVERFETCH: u32 = 4;

pub fn wanted(request: &SearchRequest) -> u32 {
    match request.limit {
        0 => DEFAULT_LIMIT,
        asked => asked.min(MAX_LIMIT),
    }
}

pub fn asked(request: &SearchRequest) -> u32 {
    wanted(request).saturating_mul(OVERFETCH).min(MAX_LIMIT)
}

fn address(filter: &str, order: Option<&str>, top: u32) -> String {
    let mut url = Url::parse(&format!("https://{}{}", HOST, PATH))
        .expect("catalogue address is built from constants");
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

pub fn search(request: &SearchRequest, floor: i64) -> String {
    address(&filter(request, floor), Some("ContentDate/Start desc"), asked(request))
}

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
    if !tile.is_empty() {
        terms.push(format!(
            "Attributes/OData.CSC.StringAttribute/any(a:a/Name eq 'tileId'              and a/OData.CSC.StringAttribute/Value eq {})",
            literal(tile)
        ));
    }
    address(&terms.join(" and "), None, siblings_top(datatake))
}

pub fn siblings_top(datatake: &str) -> u32 {
    match datatake.is_empty() {
        true => 30,
        false => 300,
    }
}

pub fn locate(name: &str) -> String {
    address(&format!("Name eq {}", literal(name)), None, 1)
}

pub const FRESH_DAYS: i64 = 2;

fn filter(request: &SearchRequest, floor: i64) -> String {
    let mut terms: Vec<String> = Vec::new();

    if !request.mission.is_empty() {
        terms.push(format!("Collection/Name eq {}", literal(&request.mission)));
    }
    if !request.name.is_empty() {
        terms.push(format!("contains(Name,{})", literal(&request.name)));
    }
    if request.from <= 0 && request.to <= 0 && floor > 0 {
        terms.push(format!("ContentDate/Start gt {}", super::time::format(floor)));
    }
    match request.area.as_ref() {
        Some(area) if area.points.len() >= 3 => terms.push(format!(
            "OData.CSC.Intersects(area=geography'SRID=4326;{}')",
            polygon(area)
        )),
        Some(area) => log::warn!(target: "handlers",
            "an area of {} vertices is not an area, the search covers the whole Earth", area.points.len()),
        None => {}
    }
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

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn polygon(ring: &Ring) -> String {
    let mut points: Vec<String> = ring
        .points
        .iter()
        .map(|point| format!("{:.6} {:.6}", point.lon, point.lat))
        .collect();
    if points.first() != points.last() {
        points.push(points[0].clone());
    }
    format!("POLYGON(({}))", points.join(","))
}

pub struct Found {
    pub scenes: Vec<DataProduct>,
    pub products: usize,
}

pub fn scenes(body: &[u8]) -> anyhow::Result<Found> {
    let products = parse(body)?;
    Ok(Found { products: products.len(), scenes: scene::group(products) })
}

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
            product.unviewable = super::imagery::unviewable(
                &product.identifier,
                product.folder,
                facts.level,
                &facts.kind,
            );
            facts.framed = !product.footprint.is_empty();
            lost += usize::from(had_geometry && product.footprint.is_empty());
            (facts, product)
        })
        .collect();

    if lost > 0 {
        log::warn!(target: "catalogue",
            "geometry failed to parse for {} of {} products: they will leave the \
             results as auxiliary data", lost, parsed.len());
    }
    Ok(parsed)
}

fn facts(product: &Product) -> Facts {
    let text = |name: &str| {
        product
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| match &attribute.value {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    Facts {
        platform: text("platformShortName"),
        instrument: text("instrumentShortName"),
        datatake: match text("datatakeID") {
            said if !said.is_empty() => said,
            _ => scene::datatake_in_name(&product.name).unwrap_or_default(),
        },
        tile: text("tileId"),
        named_tile: scene::tile_in_name(&product.name).unwrap_or_default(),
        slice: text("sliceNumber"),
        orbit: text("orbitNumber"),
        second: product.content_date.as_ref().map_or(0, |date| super::time::parse(&date.start)),
        level: scene::level(&text("processingLevel")),
        kind: text("productType"),
        size: product.size,
        framed: false,
    }
}

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
        parts: Vec::new(),
        unviewable: String::new(),
    }
}

fn rings(geometry: Geometry) -> Vec<Ring> {
    let lines: Vec<Vec<Vertex>> = match geometry.kind.as_str() {
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
            (points.len() == line.len()).then(|| Ring { points: closed(points) })
        })
        .collect()
}

type Vertex = Vec<f64>;

fn place(vertex: &Vertex) -> Option<GeoPoint> {
    match vertex.as_slice() {
        [lon, lat, ..] if (-90.0..=90.0).contains(lat) && (-360.0..=360.0).contains(lon) => {
            Some(GeoPoint { lat: *lat, lon: *lon })
        }
        _ => None,
    }
}

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
