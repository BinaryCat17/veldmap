//! How the CDSE catalogue works: the OData request and response parsing.
//!
//! The catalogue is a service separate from storage: a different address, it
//! asks for no keys (metadata is open to all) and answers with JSON, not XML.
//! The two share one thing, and the essential one — `S3Path`: the path by
//! which found items are later signed and downloaded (see `s3::key`).

use crate::proto::data_provider::{DataProduct, GeoPoint, Ring, SearchRequest};
use super::scene::{self, Facts};
use url::Url;

const HOST: &str = "catalogue.dataspace.copernicus.eu";
const PATH: &str = "/odata/v1/Products";

/// How many scenes to return when the caller said nothing: a list screen plus
/// a margin for scrolling.
const DEFAULT_LIMIT: u32 = 50;
/// OData returns no more than this at once and refuses a request beyond it.
const MAX_LIMIT: u32 = 1000;

/// How many more products to ask for than scenes are needed.
///
/// The catalogue counts products while we answer with scenes, and one
/// Sentinel-1 acquisition is up to five products. Without the margin the
/// result page would come out two to five times shorter than requested.
/// Four — because beyond that what grows is not the result but the response
/// time: fifty products arrive in 3 seconds, two hundred and fifty in four.
const OVERFETCH: u32 = 4;

/// How many scenes the caller asked for. Scenes, not products: four times as
/// many go into the request (see [`OVERFETCH`]), and the two numbers must not
/// be confused — the first one decides whether the page is complete.
pub fn wanted(request: &SearchRequest) -> u32 {
    match request.limit {
        0 => DEFAULT_LIMIT,
        asked => asked.min(MAX_LIMIT),
    }
}

/// How many products go into the request — [`wanted`] with a margin for scene
/// folding. Asked twice: when building the address and when parsing the
/// response, where it shows whether the window ran out of products or we
/// simply asked for too few.
pub fn asked(request: &SearchRequest) -> u32 {
    wanted(request).saturating_mul(OVERFETCH).min(MAX_LIMIT)
}

/// The catalogue request address: all three requests share one and differ only
/// in the filter, the ordering (`order`, `None` — as the catalogue returns
/// them), and how many products to take.
///
/// Built with `Url`, not by formatting: the filter contains quotes, brackets,
/// and spaces, and encoding them in place means forgetting once.
///
/// Attributes are always requested: mission, product type, and cloud cover
/// live only there, and showing the found items without them means showing
/// bare file names. It costs about four kilobytes per scene, and picking
/// individual attributes is not possible — the catalogue rejects a nested
/// filter on `$expand`.
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

/// The search request address.
///
/// `floor` is the lower time bound we set ourselves when the caller named none
/// (unix seconds; 0 — do not set one). It is not about the results but about
/// speed: see [`FRESH_DAYS`].
pub fn search(request: &SearchRequest, floor: i64) -> String {
    // Freshest first: a week-old scene is wanted more often than a decade-old
    // one.
    address(&filter(request, floor), Some("ContentDate/Start desc"), asked(request))
}

/// The address of the request for the sibling packages of the same
/// acquisition — the second step after [`locate`].
///
/// Asked by acquisition time, not by name: package names diverge in the tail
/// (each package has its own checksum).
///
/// The window width follows the folding rule (`scene::acquisition`), because
/// asking narrower than one folds means not seeing in the answer the part one
/// meant to attach.
///
/// One second either side — where there is no datatake. In all such rules the
/// acquisition second stands inside the key itself, that is, parts of one
/// acquisition provably start at the same instant; exact equality would do,
/// but there is no way to ask for it — products carry fractional seconds while
/// we can only ask in whole seconds (see `time::format`).
///
/// Ten seconds — where a datatake exists. Such parts are cut separately and
/// start at different times: raw data four seconds before processed, combined
/// one and a half, — while the "by slice" rule puts no limit on time at all.
/// It has no bound of its own, and it is taken from the neighboring rule
/// (`scene::BESIDE_A_SLICE_S`), which with the same ten seconds attaches
/// unnumbered parts to a slice: it covers the measured spread with room to
/// spare and falls short of the neighboring slice standing twenty-five seconds
/// away.
///
/// Datatake, not slice number: the catalogue does not give a number to every
/// part (`IW_OCN__2S` has none at all), and they are attached with the same
/// window anyway. Were we to ask by number, the same acquisition would open
/// differently depending on which of its parts was asked about.
///
/// Extra items in the response are filtered out by comparing acquisition keys —
/// that happens in `cdse::on_http_result`, and it is exact
/// (`scene::acquisition`).
///
/// The collection in the filter is not for selection but for the index:
/// without it the catalogue sorts the whole archive — see [`FRESH_DAYS`].
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
    // The tile, if the catalogue named one. For Sentinel-2 the one-second
    // window covers not the packages of a single tile but the entire imaging
    // row — nearly three hundred products (verified against the live
    // catalogue), — and the wanted packages would be lost behind the result
    // ceiling at random. The tile narrows the same window to a handful.
    if !tile.is_empty() {
        terms.push(format!(
            "Attributes/OData.CSC.StringAttribute/any(a:a/Name eq 'tileId'              and a/OData.CSC.StringAttribute/Value eq {})",
            literal(tile)
        ));
    }
    address(&terms.join(" and "), None, siblings_top(datatake))
}

/// The result ceiling for the sibling request.
///
/// It follows the window and by the same measure: for an acquisition with a
/// datatake the window is ten times wider and the ceiling ten times higher —
/// otherwise the density margin the one-second request relied on would be cut
/// along with the widening. Hitting the ceiling here means losing the wanted
/// part at random: we do not set the response order.
///
/// The lower bound is not "an acquisition has a handful of parts": there can
/// be dozens. For Sentinel-5P a dozen gases fall into the same second; for
/// cells of one Sentinel-1 RTC swath the second is shared by the whole swath.
///
/// A separate value because hitting the ceiling must be noticed, and only
/// someone who knows where the ceiling is can notice it
/// (`cdse::on_http_result`).
pub fn siblings_top(datatake: &str) -> u32 {
    match datatake.is_empty() {
        true => 30,
        false => 300,
    }
}

/// The address of a single-product request by exact name — the reverse of
/// search: the product root name is already known from the storage key, and
/// the product itself with its footprint and attributes is what is needed.
/// Exact equality, not `contains`: the product name is the catalogue key, and
/// a substring match here would mean somebody else's product.
pub fn locate(name: &str) -> String {
    address(&format!("Name eq {}", literal(name)), None, 1)
}

/// How many days back a request without a lower bound looks.
///
/// The bound here is not about the results but about speed, and it is
/// mandatory. A catalogue query without a lower time bound sorts the whole
/// archive, and this is not "slightly slower": an empty request answers in 21
/// seconds, an area request in a minute, a cloud-cover request never answers.
/// With the bound the same three take 3, 6, and 1.5 seconds.
///
/// The bound we set is not final: if the page did not fill, the request is
/// repeated without it (see `cdse::on_http_result`), and then the slow answer
/// is already paid for by real work.
pub const FRESH_DAYS: i64 = 2;

/// Request conditions joined with `and`. An empty field adds no condition, so
/// a request without a single filled field is "everything", and only `$top`
/// keeps it from being unbounded.
fn filter(request: &SearchRequest, floor: i64) -> String {
    let mut terms: Vec<String> = Vec::new();

    if !request.mission.is_empty() {
        terms.push(format!("Collection/Name eq {}", literal(&request.mission)));
    }
    if !request.name.is_empty() {
        terms.push(format!("contains(Name,{})", literal(&request.name)));
    }
    // Our own bound — only when the caller named none of their own. The upper
    // bound counts on par with the lower one: "everything up to 2020" with
    // "newer than the day before yesterday" appended is an empty intersection
    // by construction, and the first attempt goes to the network knowingly
    // empty-handed.
    if request.from <= 0 && request.to <= 0 && floor > 0 {
        terms.push(format!("ContentDate/Start gt {}", super::time::format(floor)));
    }
    // Fewer than three points is not an area: the catalogue treats such a
    // polygon as a bad request and refuses the whole search. This cannot stay
    // silent: a dropped condition means not "slightly wider" but "the whole
    // Earth", and the results after it look plausible — the other conditions
    // are still there.
    match request.area.as_ref() {
        Some(area) if area.points.len() >= 3 => terms.push(format!(
            "OData.CSC.Intersects(area=geography'SRID=4326;{}')",
            polygon(area)
        )),
        Some(area) => log::warn!(target: "handlers",
            "an area of {} vertices is not an area, the search covers the whole Earth", area.points.len()),
        None => {}
    }
    // Time — by acquisition start and without quotes: a date literal in OData
    // is not a string.
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

/// A quoted string. An inner quote is doubled — that is how OData escapes it,
/// and without this a name with an apostrophe would break the filter
/// expression.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A ring in WKT. The coordinate order there is the reverse of ours —
/// longitude first, — and closure is mandatory.
fn polygon(ring: &Ring) -> String {
    let mut points: Vec<String> = ring
        .points
        .iter()
        // Six digits are enough: about ten centimeters at the equator, while
        // search areas are defined far coarser.
        .map(|point| format!("{:.6} {:.6}", point.lon, point.lat))
        .collect();
    if points.first() != points.last() {
        points.push(points[0].clone());
    }
    format!("POLYGON(({}))", points.join(","))
}

/// What the catalogue answered to a search.
///
/// Only a product count, but it cannot be dispensed with: a short page of
/// scenes means two different things. Either the window ran out of products —
/// then the search must go wider; or a full page of them arrived and fewer
/// scenes came out only because one acquisition is several products — and then
/// widening the window is pointless, there are more.
pub struct Found {
    pub scenes: Vec<DataProduct>,
    pub products: usize,
}

/// Parses the catalogue response into scenes: products of one acquisition fold
/// into one, auxiliary data is filtered out (see scene.rs).
pub fn scenes(body: &[u8]) -> anyhow::Result<Found> {
    let products = parse(body)?;
    Ok(Found { products: products.len(), scenes: scene::group(products) })
}

/// Parses the catalogue response into products as returned — along with the
/// facts from which scenes are folded.
///
/// Separate from [`scenes`] it is needed where a single known product is asked
/// about: scene folding answers with the selected package, and whether the
/// asked-for one was found can no longer be told from such an answer.
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
            // The processing level is known only to the catalogue, and the
            // list of readable formats only to `imagery`: the two meet right
            // here.
            product.unviewable = super::imagery::unviewable(
                &product.identifier,
                product.folder,
                facts.level,
                &facts.kind,
            );
            // "A scene or auxiliary data" is decided by the outline, and there
            // is exactly one outline here — the one that came out as rings.
            // The presence of the field in the catalogue response is not the
            // same thing, and the price of an error here is not "a scene
            // without an outline" but "no scene": scene folding treats an
            // outline-less product as auxiliary data and drops it
            // (`scene::group`). Hence the count is kept out loud — see `lost`
            // below.
            facts.framed = !product.footprint.is_empty();
            lost += usize::from(had_geometry && product.footprint.is_empty());
            (facts, product)
        })
        .collect();

    // One line per response, not per product: usually the format is wrong,
    // that is, all of them at once, and a thousand identical lines would say
    // the same thing.
    if lost > 0 {
        log::warn!(target: "catalogue",
            "geometry failed to parse for {} of {} products: they will leave the \
             results as auxiliary data", lost, parsed.len());
    }
    Ok(parsed)
}

/// The catalogue facts by which products fold into scenes. They live in the
/// attributes and are fetched from there right here — the rules themselves
/// know nothing about them.
fn facts(product: &Product) -> Facts {
    let text = |name: &str| {
        product
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| match &attribute.value {
                // The datatake and orbit come from the catalogue as numbers,
                // the tile as a string — all are needed as text because they
                // serve as part of the key.
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    Facts {
        platform: text("platformShortName"),
        instrument: text("instrumentShortName"),
        // The catalogue does not report a datatake for the whole archive;
        // what it did not say is stated in the name itself
        // (see `scene::datatake_in_name`).
        datatake: match text("datatakeID") {
            said if !said.is_empty() => said,
            _ => scene::datatake_in_name(&product.name).unwrap_or_default(),
        },
        // The catalogue does not name a tile for everything: for Sentinel-1
        // RTC cells `tileId` is empty while the cell itself stands at the
        // start of the name. It travels in a separate field — it names the
        // acquisition but does not go into the catalogue request
        // (see `Facts`).
        tile: text("tileId"),
        named_tile: scene::tile_in_name(&product.name).unwrap_or_default(),
        slice: text("sliceNumber"),
        orbit: text("orbitNumber"),
        second: product.content_date.as_ref().map_or(0, |date| super::time::parse(&date.start)),
        level: scene::level(&text("processingLevel")),
        kind: text("productType"),
        size: product.size,
        // Set in [`parse`]: the outline is the parsed rings, not a field in
        // the catalogue response.
        framed: false,
    }
}

/// Extracts the refusal explanation from the response. The catalogue keeps it
/// in `detail`, and showing it is far more useful than a bare status code:
/// "bad filter" and "no such collection" look alike.
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

    // The catalogue path starts with a slash while our identifier does not:
    // with it `s3::key` would see an empty bucket name.
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
        // Parts are filled in by scene folding — it alone sees the neighbors,
        // — while the refusal reason needs the processing level and is set in
        // [`parse`].
        parts: Vec::new(),
        unviewable: String::new(),
    }
}

/// GeoJSON geometry to rings.
///
/// Polygon holes arrive here on par with the outer outline: they are drawn the
/// same way, and there is nothing to fill here — rings are drawn as lines.
///
/// Polylines are rings too, and for the same reason: they are drawn as lines,
/// and closure is not needed for that. In today's CDSE archive nobody returns
/// them — a nadir altimeter outline is a polygon too — but GeoJSON allows such
/// geometry, and dropped, it would leave the scene without a single vertex
/// while `framed` claims there is an outline (see [`parse`]).
fn rings(geometry: Geometry) -> Vec<Ring> {
    // Outward all shapes arrive in one form — a list of polylines: a polygon
    // is a closed polyline, and the difference between "cut in two across the
    // seam" and "two polylines" does not matter to the renderer.
    let lines: Vec<Vec<Vertex>> = match geometry.kind.as_str() {
        // A scene across the 180th meridian the catalogue returns cut, and
        // then there are several polygons.
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
            // The ring leaves whole or not at all. A bad vertex dropped one by
            // one silently alters the shape: a four-cornered scene keeps
            // three, and on the globe it lands not as the binding square but
            // as a crude extent. Dropped whole, the ring lands in the
            // unparseable-geometry count — but only if it was the product's
            // only ring: the count there goes by products without a single
            // ring (see [`parse`]), and for a scene cut in two across the seam
            // the loss of half passes silently.
            (points.len() == line.len()).then(|| Ring { points: closed(points) })
        })
        .collect()
}

/// A GeoJSON vertex. A list, not a pair: RFC 7946 allows a third coordinate,
/// and a pair would reject such a vertex at parse time — taking the product's
/// whole geometry with it, because parsing ends in `unwrap_or_default`. The
/// scene would leave the results as auxiliary data (see [`parse`]) over a
/// number that is not even needed here.
type Vertex = Vec<f64>;

/// The place of a vertex. In GeoJSON coordinates go longitude first, and the
/// height, if recorded, is dropped: the outline lies on the surface and has
/// no height anywhere downstream.
///
/// A vertex shorter than a pair names no place, and a silently substituted
/// zero would carry the outline off to the Gulf of Guinea. What to do with
/// such a ring is decided by [`rings`].
fn place(vertex: &Vertex) -> Option<GeoPoint> {
    match vertex.as_slice() {
        // A vertex that is not on Earth is not a vertex. The numbers arrive
        // here from the catalogue as-is, and unchecked they cost dearly not in
        // accuracy but in time: the outline is traced one degree per step, and
        // an edge a million degrees long is drawn in a billion steps.
        [lon, lat, ..] if (-90.0..=90.0).contains(lat) && (-360.0..=360.0).contains(lon) => {
            Some(GeoPoint { lat: *lat, lon: *lon })
        }
        _ => None,
    }
}

/// A ring without its closing vertex: in GeoJSON the last point repeats the
/// first, and that is a closure record, not a separate vertex. Counting it as
/// a vertex skews everything derived from the point set — the outline's middle
/// first of all: a rectangle that repeated its southern corner has its center
/// drift south.
///
/// Places are compared, not vertex records: closure is about where the vertex
/// lies. A ring whose ends are recorded at different heights is closed no less
/// than the rest, while comparing records would declare it open and the extra
/// vertex would travel on.
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
    /// Present only on a refusal.
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
