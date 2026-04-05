use veldmap_api::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse};
use log::info;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::time::SystemTime;
use aws_sigv4::http_request::{sign, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use url::Url;
use veldsdk::prost::Message;
use crate::{LocalConfig, LocalState};

const S3_HOST: &str = "eodata.dataspace.copernicus.eu";
const S3_REGION: &str = "default";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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

fn get_s3_headers(state: &LocalState, method: &str, uri: &str) -> std::collections::HashMap<String, String> {
    let signing_settings = SigningSettings::default();
    let signing_params = v4::SigningParams::builder()
        .identity(&state.identity)
        .region(S3_REGION)
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .unwrap();

    let headers = [
        ("host", S3_HOST),
        ("x-amz-content-sha256", EMPTY_SHA256),
    ];
    
    let signable_request = SignableRequest::new(
        method,
        uri,
        headers.iter().map(|(k, v)| (*k, *v)),
        aws_sigv4::http_request::SignableBody::Bytes(&[]),
    ).unwrap();

    let (instructions, _signature) = sign(signable_request, &signing_params.into()).unwrap().into_parts();
    
    let mut signed_headers = std::collections::HashMap::new();
    for (name, value) in instructions.headers() {
        signed_headers.insert(name.to_string(), value.to_string());
    }
    signed_headers.insert("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string());
    signed_headers
}

pub fn search(_state: LocalState, _request: SearchRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
    veldsdk::core::task::spawn(async move {
        // Здесь будет логика поиска через OData/OpenSearch
        // Пока возвращаем пустой результат для примера
        use veldsdk::prost::Message;
        Ok(SearchResponse { products: vec![], error: String::new() }.encode_to_vec())
    }, |u| u)
}

pub fn download(state: LocalState, request: DownloadRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
    let s3_key = request.identifier.trim_start_matches('/').trim_start_matches("eodata/").to_string();
    let url = format!("https://{}/eodata/{}", S3_HOST, s3_key);
    let uri = format!("/eodata/{}", s3_key);

    let headers = get_s3_headers(&state, "GET", &uri);

    let req_task = veldsdk::core::FsDownloadRequest {
        url,
        path: request.destination,
        headers,
    };

    veldsdk::core::raw::fs_download_task(req_task, |u| match u {
        veldsdk::core::task::TaskUpdate::Started(_) => veldsdk::core::task::TaskUpdate::Started(None),
        veldsdk::core::task::TaskUpdate::Progress(p, _) => veldsdk::core::task::TaskUpdate::Progress(p, None),
        veldsdk::core::task::TaskUpdate::Finished(Ok(_)) => {
            veldsdk::core::task::TaskUpdate::Finished(Ok(DownloadResponse { 
                success: true, 
                error: String::new(), 
                download_url: String::new(), 
            }.encode_to_vec()))
        }
        veldsdk::core::task::TaskUpdate::Finished(Err(e)) => veldsdk::core::task::TaskUpdate::Finished(Err(e)),
    })
}

pub fn list_path(state: LocalState, request: ListPathRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
    let prefix = request.path.trim_start_matches('/').trim_start_matches("eodata/").to_string();
    
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

    let mut url = Url::parse(&format!("https://{}/eodata/", S3_HOST)).unwrap();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (k, v) in &query_params {
            pairs.append_pair(k, v);
        }
    }
    let full_url = url.to_string();

    let uri_with_query = if let Some(q) = url.query() {
        format!("{}?{}", url.path(), q)
    } else {
        url.path().to_string()
    };
    
    let headers = get_s3_headers(&state, "GET", &uri_with_query);

    let req_task = veldsdk::core::HttpTaskRequest {
        url: full_url,
        method: "GET".to_string(),
        headers,
        body: Vec::new(),
    };
    
    info!("Requesting S3 list: {}", req_task.url);

    let filter_path = format!("eodata/{}", request.path.trim_start_matches('/').trim_start_matches("eodata/").trim_start_matches('/'));

    veldsdk::core::raw::http_task(req_task, move |u| {
        use veldsdk::core::task::TaskUpdate::*;
        match u {
            Started(id) => Started(id),
            Progress(p, id) => Progress(p, id),
            Finished(Ok(res)) => {
                if res.status >= 200 && res.status < 300 {
                    if res.body.is_empty() {
                         Finished(Err("Empty S3 response".to_string()))
                    } else {
                         let parsed = parse_s3_xml(res.body, Some(&filter_path));
                         info!("S3 found {} items", parsed.items.len());
                         Finished(Ok(parsed.encode_to_vec()))
                    }
                } else {
                    Finished(Err(format!("HTTP Error {}: {}", res.status, String::from_utf8_lossy(&res.body))))
                }
            }
            Finished(Err(e)) => Finished(Err(e)),
        }
    })
}

fn parse_s3_xml(body: Vec<u8>, filter_path: Option<&str>) -> ListPathResponse {
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
                    "Prefix" if in_common_prefixes => { 
                        let path = format!("eodata/{}", text);
                        if Some(path.as_str()) != filter_path {
                            items.push(path);
                        }
                    }
                    "Key" => { 
                        let path = format!("eodata/{}", text);
                        if Some(path.as_str()) != filter_path {
                            items.push(path);
                        }
                    }
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
    
    ListPathResponse { items, next_token, error: String::new() }
}

