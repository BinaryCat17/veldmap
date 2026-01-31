use extism_pdk::*;
use veldmap_rust_rpc::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse};
use veldmap_rust_rpc::host::host_log;
use serde_json::Value;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::time::SystemTime;
use aws_sigv4::http_request::{sign, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use url::Url;

fn get_config() -> Value {
    let config_val = config::get("config").unwrap_or_default();
    let config_str = config_val.unwrap_or_else(|| "{}".to_string());
    serde_json::from_str(&config_str).unwrap_or(Value::Object(serde_json::Map::new()))
}

fn get_identity() -> anyhow::Result<Identity> {
    let cfg = get_config();
    let access_key = cfg["access_key"].as_str().unwrap_or("").to_string();
    let secret_key = cfg["secret_key"].as_str().unwrap_or("").to_string();
    
    if access_key.is_empty() || secret_key.is_empty() {
        return Err(anyhow::anyhow!("S3 credentials missing"));
    }
    
    let credentials = aws_credential_types::Credentials::new(access_key, secret_key, None, None, "veldmap");
    Ok(Identity::new(credentials, None))
}

pub fn search(_request: SearchRequest) -> anyhow::Result<SearchResponse> {
    Ok(SearchResponse { products: vec![], error: String::new() })
}

pub fn download(_request: DownloadRequest) -> anyhow::Result<DownloadResponse> {
    Ok(DownloadResponse { success: false, error: "Download not implemented".into(), download_url: "".into() })
}

pub fn list_path(request: ListPathRequest) -> anyhow::Result<ListPathResponse> {
    let prefix = request.path.trim_start_matches('/').trim_start_matches("eodata/").to_string();
    let identity = get_identity()?;
    
    let host = "eodata.dataspace.copernicus.eu";
    let region = "default";
    
    let mut query_params = vec![
        ("delimiter", "/"),
        ("list-type", "2"),
        ("max-keys", "20"),
    ];
    if !prefix.is_empty() {
        query_params.push(("prefix", &prefix));
    }
    if !request.token.is_empty() {
        query_params.push(("continuation-token", &request.token));
    }
    query_params.sort_by_key(|k| k.0);

    // ВАЖНО: Используем /eodata/ в пути
    let mut url = Url::parse(&format!("https://{}/eodata/", host)).unwrap();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (k, v) in &query_params {
            pairs.append_pair(k, v);
        }
    }
    let full_url = url.to_string();

    let signing_settings = SigningSettings::default();
    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .unwrap();

    // S3 требует x-amz-content-sha256 для GET запросов
    let content_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let headers = [
        ("host", host),
        ("x-amz-content-sha256", content_sha256),
    ];
    
    let uri_with_query = if let Some(q) = url.query() {
        format!("{}?{}", url.path(), q)
    } else {
        url.path().to_string()
    };
    
    host_log(&format!("Signing request: GET {} (region: {})", uri_with_query, region));

    let signable_request = SignableRequest::new(
        "GET",
        &uri_with_query,
        headers.iter().map(|(k, v)| (*k, *v)),
        aws_sigv4::http_request::SignableBody::Bytes(&[]),
    ).unwrap();

    let (instructions, _signature) = sign(signable_request, &signing_params.into()).unwrap().into_parts();
    
    let mut req = HttpRequest::new(full_url);
    for (name, value) in instructions.headers() {
        req = req.with_header(name.to_string(), value.to_string());
    }
    // Нужно явно добавить x-amz-content-sha256 в сам запрос, если sign его не добавил в instructions
    req = req.with_header("x-amz-content-sha256".to_string(), content_sha256.to_string());

    let res = match http::request::<()>(&req, None) {
        Ok(r) => r,
        Err(e) => return Ok(ListPathResponse { items: vec![], next_token: "".into(), error: format!("Network error: {}", e) }),
    };
    
    let body = res.body();
    let status = res.status();
    
    if status != 200 && status != 0 {
        let err_msg = format!("S3 status {}: {}", status, String::from_utf8_lossy(&body));
        host_log(&err_msg);
        return Ok(ListPathResponse { items: vec![], next_token: "".into(), error: err_msg });
    }

    if body.is_empty() {
        return Ok(ListPathResponse { items: vec![], next_token: "".into(), error: "Empty response from S3".into() });
    }

    Ok(parse_s3_xml(body))
}

fn parse_s3_xml(body: Vec<u8>) -> ListPathResponse {
    let mut reader = Reader::from_reader(body.as_slice());
    reader.config_mut().trim_text(true);

    let mut items = Vec::new();
    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut in_common_prefixes = false;
    let mut next_token = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "CommonPrefixes" { in_common_prefixes = true; }
                current_tag = name;
            }
            Ok(Event::Text(e)) => {
                let text = match e.unescape() { Ok(t) => t.into_owned(), Err(_) => String::new() };
                match current_tag.as_str() {
                    "Prefix" if in_common_prefixes => { items.push(format!("eodata/{}", text)); }
                    "Key" => { items.push(format!("eodata/{}", text)); }
                    "NextContinuationToken" => { next_token = text; }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "CommonPrefixes" { in_common_prefixes = false; }
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return ListPathResponse { items: vec![], next_token: "".into(), error: format!("XML error: {}", e) },
            _ => {}
        }
        buf.clear();
    }
    
    host_log(&format!("S3 found {} items", items.len()));
    ListPathResponse { items, next_token, error: String::new() }
}
