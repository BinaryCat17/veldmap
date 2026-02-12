use veldmap_gis_api::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse};
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

pub fn search(_state: LocalState, _request: SearchRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
    veldsdk::core::task::spawn(async move {
        // Здесь будет логика поиска через OData/OpenSearch
        // Пока возвращаем пустой результат для примера
        use veldsdk::prost::Message;
        Ok(SearchResponse { products: vec![], error: String::new() }.encode_to_vec())
    }, |u| u)
}

use std::pin::Pin;

pub fn download(state: LocalState, request: DownloadRequest) -> veldsdk::core::Command<veldsdk::core::task::TaskUpdate<Vec<u8>>> {
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

    let req_task = veldsdk::core::FsDownloadRequest {
        url,
        path: request.destination,
        headers: download_headers,
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
        headers.iter().map(|(k, v)| (*k, *v)),
        aws_sigv4::http_request::SignableBody::Bytes(&[]),
    ).unwrap();

    let (instructions, _signature) = sign(signable_request, &signing_params.into()).unwrap().into_parts();
    
    let mut headers = std::collections::HashMap::new();
    for (name, value) in instructions.headers() {
        headers.insert(name.to_string(), value.to_string());
    }
    headers.insert("x-amz-content-sha256".to_string(), content_sha256.to_string());

    let req_task = veldsdk::core::HttpTaskRequest {
        url: full_url,
        method: "GET".to_string(),
        headers,
        body: Vec::new(),
    };
    
    info!("Requesting S3 list: {}", req_task.url);

    veldsdk::core::raw::http_task(req_task, |u| {
        use veldsdk::core::task::TaskUpdate::*;
        match u {
            Started(id) => {
                info!("HTTP Task Started: {:?}", id);
                Started(id)
            },
            Progress(p, id) => {
                // info!("HTTP Task Progress: {}", p); // Too noisy
                Progress(p, id)
            },
            Finished(Ok(res)) => {
                info!("HTTP Task Finished. Status: {}, Body size: {}", res.status, res.body.len());
                if res.status >= 200 && res.status < 300 {
                    if res.body.is_empty() {
                         info!("HTTP Body is empty");
                         Finished(Err("Empty S3 response".to_string()))
                    } else {
                         info!("Parsing XML...");
                         let parsed = parse_s3_xml(res.body);
                         info!("XML Parsed. Items: {}", parsed.items.len());
                         Finished(Ok(parsed.encode_to_vec()))
                    }
                } else {
                    let err_msg = format!("HTTP Error {}: {}", res.status, String::from_utf8_lossy(&res.body));
                    info!("{}", err_msg);
                    Finished(Err(err_msg))
                }
            }
            Finished(Err(e)) => {
                info!("HTTP Task Finished with Error: {}", e);
                Finished(Err(e))
            },
        }
    })
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
