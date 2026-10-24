//! CDSE bucket protocol: addressing, signing, listing parsing.
//!
//! Everything this module knows about how that side is built — and nothing
//! about the bus. Handlers (cdse.rs) receive a ready request and a parsed
//! response.
//!
//! The main rule of the module: the address and the headers never diverge.
//! SigV4 signs a specific path, so building the URL in one place and the
//! signature in another is forbidden: once they diverge, they yield 403 (the
//! signature is for another path) or 404 (the address is for another object).
//! Hence `Request` is a pair, and it is created only here.

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
/// The bucket — the first path segment: at CDSE this is S3 with path-style
/// addressing.
const BUCKET: &str = "eodata";
/// sha256 of an empty body: all our requests are GET, none of them has a body.
/// An S3 header signature must carry this header.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// How many objects to request at once. Listing is paged, continuation goes
/// through a continuation-token (see `Listing::next_token`): a page here is the
/// size of one storage response, not what the user sees.
const PAGE_SIZE: &str = "200";

/// A signed request: an address and the headers for it.
pub struct Request {
    pub url: String,
    pub headers: HashMap<String, String>,
}

/// A parsed listing of one level.
pub struct Listing {
    /// Folders (with a trailing `/`) and objects mixed together, in the form
    /// this module's functions accept.
    pub entries: Vec<Entry>,
    /// Empty — this is the last page.
    pub next_token: String,
}

/// A listing entry: the key and what the catalogue reported about it.
pub struct Entry {
    pub identifier: String,
    pub size: u64,
    pub modified: i64,
}

/// The object key in the bucket from a product identifier.
///
/// The identifier is the path together with the bucket name
/// (`eodata/Sentinel-2/…`): in this form it is shown in the UI, stored in the
/// library, and fit for navigation. The key lives *under* the bucket, so it
/// goes into the request without the prefix, and the bucket is added by the
/// address.
///
/// Conversion between the two forms happens only here and in [`identifier`],
/// never inline in handlers: a forgotten prefix slice yields
/// `/eodata/eodata/…`, i.e. 404 on network viewing while the same file
/// downloads fine.
fn key(identifier: &str) -> &str {
    let path = identifier.trim_start_matches('/');
    match path.strip_prefix(BUCKET) {
        Some("") => "",
        // Exactly a path segment, not the start of a name: an object with the
        // key "eodata-backup/…" has no prefix to strip.
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => path,
    }
}

/// The reverse conversion: a key from an S3 response → an identifier for
/// everyone else.
fn identifier(key: &str) -> String {
    format!("{}/{}", BUCKET, key)
}

/// The product is a single object, not a directory. Storage keeps products in
/// two ways: as an expanded directory (.SAFE, .SEN3, granules without a
/// suffix) or as a single archive/file (auxiliary data, climate data), and the
/// CDSE catalogue does not report this difference — it is visible only from
/// the container format name. A mistake on a single format outside the list is
/// safe: "entering" such a product shows an empty folder and "up", not a 404.
pub fn is_single_object(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    // A raster is also a single object, and its extension list lives with the
    // overlay: should these two enumerations diverge, a file product
    // `…/tile.TIF` would be treated as a folder, paged by prefix, and answer
    // "no file in storage" — while its view icon is already drawn above it.
    if super::imagery::is_raster(name) {
        return true;
    }
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "tgz" | "zip" | "tar" | "gz" | "n1" | "e1" | "e2" | "hdf" | "dbl"
    )
}

/// The product root that contains the key. `None` — the key lies on the path
/// to scenes, not inside a scene: there is neither a container suffix nor a
/// date above it.
///
/// The product boundary is drawn by one rule for two cases, because the case
/// is one: where the path ends and the scene begins.
///  * a directory container suffix — .SAFE for Sentinel-1/2, .SEN3 for
///    Sentinel-3: the finest (leftmost) such segment is the root;
///  * without a suffix — the first segment right under the acquisition date
///    triple (`…/YYYY/MM/DD/<scene>/…`): this is how Landsat and RTC lie.
///
/// The absence of an answer is exactly `None`, not "the key itself": "its own
/// root" and "there is no scene here" are different things, and merged into
/// one they turn a year folder into a scene. Whoever needs a fallback names it
/// on their own side.
pub fn product_root(identifier: &str) -> Option<&str> {
    let trimmed = identifier.trim_end_matches('/');
    let mut offset = 0;
    // Segments run left to right, and with them the path up to the end of each:
    // the root is returned as a slice, not assembled from parts.
    for segment in trimmed.split('/') {
        let end = offset + segment.len();
        let suffix = segment.rsplit_once('.').map(|(_, suffix)| suffix.to_ascii_lowercase());
        if matches!(suffix.as_deref(), Some("safe" | "sen3")) {
            return Some(&trimmed[..end]);
        }
        // A date stands above this segment — so this one is the scene.
        if under_date(&trimmed[..offset.saturating_sub(1)]) {
            return Some(&trimmed[..end]);
        }
        offset = end + 1;
    }
    None
}

/// The path ends with a "year/month/day" triple — the place where storage puts
/// the scenes themselves. Below it there are no path folders left, so
/// everything lying right here is a product.
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

/// GET of an object whole or by range — Range is not part of the signature and
/// is added by the transport (network).
pub fn object(identity: &Identity, identifier: &str) -> Request {
    signed(identity, &format!("/{}/{}", BUCKET, key(identifier)), &[])
}

/// The same signature again — by address, not by identifier: network asks for
/// it, having nothing left of the object but an address. A foreign address is
/// not signed: the keys belong to our own storage, and a signature over a
/// foreign path would travel there along with them; without TLS likewise, for
/// the same reason.
pub fn resign(identity: &Identity, url: &str) -> Option<Request> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(HOST) {
        return None;
    }
    Some(signed(identity, parsed.path(), &[]))
}

/// A listing of one level: folders are returned as CommonPrefixes, not as an
/// expansion of the whole subtree — hence the delimiter.
pub fn listing(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, true)
}

/// A listing of the whole subtree: without a delimiter S3 expands every key
/// under the prefix. This finds a product's rasters with one request per page
/// — walking a .SAFE level by level would be four or five requests per
/// granule.
pub fn listing_deep(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, false)
}

/// A ListObjectsV2 page. Level and subtree are the very same request to the
/// bucket, and it is signed the same way; the whole difference between them
/// is in the parameters.
fn page(identity: &Identity, path: &str, token: &str, by_level: bool) -> Request {
    signed(identity, &format!("/{}/", BUCKET), &query(key(path), token, by_level))
}

/// Listing page parameters.
///
/// The delimiter is exactly the choice between "show what lies right here" and
/// "expand everything under the prefix" — a level differs from a subtree in
/// nothing else. Empty prefix and continuation-token are not sent at all: the
/// bucket root has no prefix, and the token appears only from the second page,
/// and empty it would be an unusable continuation for the storage.
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

/// The only place where the "address + signature" pair is born.
///
/// The signature lives in the headers, and it lives as long as the storage
/// accepts `x-amz-date`: a quarter of an hour. A signature in the address
/// (presign, `X-Amz-*` in the query string) the CDSE storage rejects with
/// `InvalidAccessKeyId` at any lifetime (ADR 0005), so the life of the public
/// resource is exactly the life of the headers.
fn signed(identity: &Identity, path: &str, query: &[(&str, &str)]) -> Request {
    // Through Url, not by formatting: exactly the query string that goes to
    // the network is signed — with all its percent encoding.
    let mut url = Url::parse(&format!("https://{}{}", HOST, path))
        .expect("the bucket address is built from a constant and a key");
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

    // SigningSettings defaults are not for S3: it canonicalizes the path with
    // single percent encoding and without segment normalization, while the
    // defaults give double encoding and normalization. On the safe alphabet of
    // Sentinel keys the canonicalizations coincide, but a key with a space,
    // "+" or `..` would yield 403.
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
        .expect("the signing params are fully populated");

    let headers = [("host", HOST), ("x-amz-content-sha256", EMPTY_SHA256)];
    let request = SignableRequest::new(
        "GET",
        &signable_path,
        headers.iter().map(|(name, value)| (*name, *value)),
        SignableBody::Bytes(&[]),
    ).expect("the signable request is built from correct strings");

    let (instructions, _signature) = sign(request, &params.into())
        .expect("the signature is computed locally and cannot fail")
        .into_parts();

    let mut signed = HashMap::new();
    for (name, value) in instructions.headers() {
        signed.insert(name.to_string(), value.to_string());
    }
    // The header takes part in the signature, but the instructions return only
    // the computed ones — the signed header must be inserted manually.
    signed.insert("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string());

    Request { url: url.to_string(), headers: signed }
}

/// Parsing of a ListObjectsV2 response.
///
/// `requested` is the path that was asked for: S3 returns it to itself in the
/// listing, and in a folder's content list the folder has no business being
/// its own element.
pub fn parse_listing(body: &[u8], requested: &str) -> anyhow::Result<Listing> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut entries: Vec<Entry> = Vec::new();
    let mut next_token = String::new();
    let mut buf = Vec::new();
    let mut tag = String::new();
    let mut in_common_prefixes = false;
    // The object being assembled: Key, Size and LastModified arrive as
    // separate events, and before `</Contents>` the element is incomplete.
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
                        // A key with a trailing slash is not an object but a
                        // placeholder the storage uses to mark a folder.
                        // Folders in the listing are answered by common
                        // prefixes, and the same folder arrives as one, while
                        // in a deep walk folders do not exist at all: a kept
                        // placeholder would become a zero-size file that then
                        // gets "downloaded".
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
                // Decoding and unescaping entities (&amp;, &#x2F;) are two
                // different steps: the first removes the document encoding,
                // the second works on the text itself. Entities in bucket keys
                // are real — slashes and ampersands in product names arrive
                // escaped.
                let raw = e.xml10_content().unwrap_or_default();
                let text = match quick_xml::escape::unescape(&raw) {
                    Ok(unescaped) => unescaped.into_owned(),
                    Err(_) => raw.into_owned(),
                };
                match (tag.as_str(), &mut object) {
                    // Prefix occurs outside CommonPrefixes too — there it is
                    // the echo of the request, not a listing element. A folder
                    // has neither size nor time: behind it stands a common
                    // prefix of keys, not an object.
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

/// S3 returns the requested path itself as an element of the listing — in a
/// folder's content list the folder has no business being itself.
fn push(entries: &mut Vec<Entry>, entry: Entry, requested: &str) {
    if entry.identifier != requested && !entry.identifier.is_empty() {
        entries.push(entry);
    }
}
