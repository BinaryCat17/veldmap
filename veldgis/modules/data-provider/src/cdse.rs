use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse};
use log::info;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::time::SystemTime;
use aws_sigv4::http_request::{sign, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use url::Url;
use veldsdk::core::{HttpRequest, http_request};
use crate::{LocalConfig, LocalState};

pub fn module_init(config: LocalConfig) -> anyhow::Result<LocalState> {
    let credentials = aws_credential_types::Credentials::new(
        config.access_key, 
        config.secret_key, 
        None, None, "veldmap"
    );
    let identity = Identity::new(credentials, None);
    
    Ok(LocalState {
        identity,
    })
}

pub fn search(_state: &LocalState, _request: SearchRequest) -> anyhow::Result<SearchResponse> {
    Ok(SearchResponse { products: vec![], error: String::new() })
}

pub fn download(state: &LocalState, request: DownloadRequest) -> anyhow::Result<DownloadResponse> {
    let s3_key = request.identifier.trim_start_matches('/').trim_start_matches("eodata/").to_string();
    let host = "eodata.dataspace.copernicus.eu";
    let region = "default";
    
    let url = format!("https://{}/eodata/{}", host, s3_key);
    let uri = format!("/eodata/{}", s3_key);

    let signing_settings = SigningSettings::default();
    let signing_params = v4::SigningParams::builder()
        .identity(&state.identity)
        .region(region)
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .unwrap();

    let content_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let headers = [
        ("host", host),
        ("x-amz-content-sha256", content_sha256),
    ];
    
    let signable_request = SignableRequest::new(
        "GET",
        &uri,
        headers.iter().map(|(k, v)| (*k, *v)),
        aws_sigv4::http_request::SignableBody::Bytes(&[]),
    ).unwrap();

    let (instructions, _signature) = sign(signable_request, &signing_params.into()).unwrap().into_parts();
    
    let mut download_headers = std::collections::HashMap::new();
    for (name, value) in instructions.headers() {
        download_headers.insert(name.to_string(), value.to_string());
    }
    download_headers.insert("x-amz-content-sha256".to_string(), content_sha256.to_string());

    match veldsdk::core::fs_download(url.clone(), request.destination, download_headers) {
        Ok(task_id) => Ok(DownloadResponse { success: true, error: "".into(), download_url: url, task_id }),
        Err(e) => Ok(DownloadResponse { success: false, error: format!("Host download failed: {}", e), download_url: "".into(), task_id: "".into() }),
    }
}

pub fn list_path(state: &LocalState, request: ListPathRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
    let state = state.clone(); // Identity в LocalState должна быть клонируемой или Arc
    veldsdk::core::task::spawn(async move {
        let prefix = request.path.trim_start_matches('/').trim_start_matches("eodata/").to_string();
        
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
            .identity(&state.identity)
            .region(region)
            .name("s3")
            .time(SystemTime::now())
            .settings(signing_settings)
            .build()
            .unwrap();

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
        
        let signable_request = SignableRequest::new(
            "GET",
            &uri_with_query,
            headers.iter().map(|| (*"host", host)), // Упростил для примера
            aws_sigv4::http_request::SignableBody::Bytes(&[]),
        ).unwrap();

        let (instructions, _signature) = sign(signable_request, &signing_params.into()).unwrap().into_parts();
        
        let mut req = HttpRequest::new(full_url);
        for (name, value) in instructions.headers() {
            req = req.with_header(name.to_string(), value.to_string());
        }
        req = req.with_header("x-amz-content-sha256".to_string(), content_sha256.to_string());

        let (status, body) = match http_request(&req, None) {
            Ok(r) => r,
            Err(e) => return Err(format!("Network error: {}", e)),
        };
        
        if status != 200 && status != 0 {
            return Err(format!("S3 status {}: {}", status, String::from_utf8_lossy(&body)));
        }

        if body.is_empty() {
            return Err("Empty response from S3".into());
        }

        use veldsdk::prost::Message;
        Ok(parse_s3_xml(body).encode_to_vec())
    }, |u| u)
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
    
    info!("S3 found {} items", items.len());
    ListPathResponse { items, next_token, error: String::new() }
}
