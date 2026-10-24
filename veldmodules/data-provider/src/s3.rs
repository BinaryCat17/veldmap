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
const BUCKET: &str = "eodata";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const PAGE_SIZE: &str = "200";

pub struct Request {
    pub url: String,
    pub headers: HashMap<String, String>,
}

pub struct Listing {
    pub entries: Vec<Entry>,
    pub next_token: String,
}

pub struct Entry {
    pub identifier: String,
    pub size: u64,
    pub modified: i64,
}

fn key(identifier: &str) -> &str {
    let path = identifier.trim_start_matches('/');
    match path.strip_prefix(BUCKET) {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => path,
    }
}

fn identifier(key: &str) -> String {
    format!("{}/{}", BUCKET, key)
}

pub fn is_single_object(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if super::imagery::is_raster(name) {
        return true;
    }
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "tgz" | "zip" | "tar" | "gz" | "n1" | "e1" | "e2" | "hdf" | "dbl"
    )
}

pub fn product_root(identifier: &str) -> Option<&str> {
    let trimmed = identifier.trim_end_matches('/');
    let mut offset = 0;
    for segment in trimmed.split('/') {
        let end = offset + segment.len();
        let suffix = segment.rsplit_once('.').map(|(_, suffix)| suffix.to_ascii_lowercase());
        if matches!(suffix.as_deref(), Some("safe" | "sen3")) {
            return Some(&trimmed[..end]);
        }
        if under_date(&trimmed[..offset.saturating_sub(1)]) {
            return Some(&trimmed[..end]);
        }
        offset = end + 1;
    }
    None
}

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

pub fn object(identity: &Identity, identifier: &str) -> Request {
    signed(identity, &format!("/{}/{}", BUCKET, key(identifier)), &[])
}

pub fn resign(identity: &Identity, url: &str) -> Option<Request> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(HOST) {
        return None;
    }
    Some(signed(identity, parsed.path(), &[]))
}

pub fn listing(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, true)
}

pub fn listing_deep(identity: &Identity, path: &str, token: &str) -> Request {
    page(identity, path, token, false)
}

fn page(identity: &Identity, path: &str, token: &str, by_level: bool) -> Request {
    signed(identity, &format!("/{}/", BUCKET), &query(key(path), token, by_level))
}

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

// CDSE S3 rejects presigned query URLs (`X-Amz-*`, `InvalidAccessKeyId` at any lifetime): the signature goes in the headers
// and is accepted for a quarter of an hour (`x-amz-date`).
fn signed(identity: &Identity, path: &str, query: &[(&str, &str)]) -> Request {
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

    // aws-sigv4 defaults double-encode and normalize the path; S3 canonicalization does neither,
    // so a key with a space, '+' or `..` would yield 403 with the defaults.
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
    signed.insert("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string());

    Request { url: url.to_string(), headers: signed }
}

pub fn parse_listing(body: &[u8], requested: &str) -> anyhow::Result<Listing> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut entries: Vec<Entry> = Vec::new();
    let mut next_token = String::new();
    let mut buf = Vec::new();
    let mut tag = String::new();
    let mut in_common_prefixes = false;
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
                let raw = e.xml10_content().unwrap_or_default();
                let text = match quick_xml::escape::unescape(&raw) {
                    Ok(unescaped) => unescaped.into_owned(),
                    Err(_) => raw.into_owned(),
                };
                match (tag.as_str(), &mut object) {
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

fn push(entries: &mut Vec<Entry>, entry: Entry, requested: &str) {
    if entry.identifier != requested && !entry.identifier.is_empty() {
        entries.push(entry);
    }
}
