use crate::proto::data_provider::{
    DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest,
    ListPathRequest, ListPathResponse
};
use log::info;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::time::SystemTime;
use aws_sigv4::http_request::{sign, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use url::Url;
use super::{Config, State};

const S3_HOST: &str = "eodata.dataspace.copernicus.eu";
const S3_REGION: &str = "default";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub fn module_init(config: Config) -> anyhow::Result<State> {
    let credentials = aws_credential_types::Credentials::new(
        config.access_key, 
        config.secret_key, 
        None, None, "veldmap"
    );
    let identity = Identity::new(credentials, None);
    
    Ok(State { 
        identity,
        tasks: veldsdk::TaskTracker::new(),
        pending_http: veldsdk::Correlator::new(),
    })
}

//  inputs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_search(
    _state: &mut State, 
    _request: crate::proto::data_provider::SearchRequest
) {
    // TODO: Implement search via OData/OpenSearch
    info!("Search requested (not implemented)");
}

pub fn on_download(
    state: &mut State, 
    request: DownloadRequest
) {
    let task_id = veldsdk::generate_id();
    let s3_key = request.identifier.trim_start_matches('/').trim_start_matches("eodata/").to_string();
    let destination = request.destination.clone();
    let identifier = request.identifier.clone();
    
    // Publish started immediately
    crate::emit::on_download_started(&DownloadStarted {
        task_id: task_id.clone(),
        identifier: identifier.clone(),
        destination: destination.clone(),
    });
    
    let url = format!("https://{}/eodata/{}", S3_HOST, s3_key);
    let uri = format!("/eodata/{}", s3_key);
    let headers = get_s3_headers(state, "GET", &uri);

    let req_task = veldsdk::proto::network::FsDownloadRequest {
        url,
        path: destination,
        headers,
        correlation_id: task_id.clone(),
    };

    state.tasks.track(task_id);
    crate::calls::network::on_fs_download(&req_task);
}

pub fn on_cancel_download(
    state: &mut State,
    request: CancelDownloadRequest
) {
    // Отменяем только задачи, которые этот модуль реально запустил.
    // Отмену выполнит платформа (топик tasks/cancel, проверка lease);
    // доменное уведомление — в on_task_finished, когда отмена случится.
    state.tasks.cancel(&request.task_id);
}

/// Терминальное событие платформы. Доменный результат (fs_download_result)
/// приходит первым и снимает задачу с учёта; сюда доходят только отмены —
/// при abort доменного результата не было, уведомляем подписчиков сами.
pub fn on_task_finished(
    state: &mut State,
    event: veldsdk::proto::tasks::TaskFinished
) {
    if !event.cancelled || !state.tasks.finish(&event.task_id) {
        return;
    }
    crate::emit::on_downloaded(&Downloaded {
        task_id: event.task_id,
        success: false,
        error: "Cancelled by user".to_string(),
    });
}

pub fn on_list_path(
    state: &mut State, 
    request: ListPathRequest
) {
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
    
    let headers = get_s3_headers(state, "GET", &uri_with_query);
    let correlation_id = state.pending_http.begin(request.path);

    let req_task = veldsdk::proto::network::HttpTaskRequest {
        url: full_url,
        method: "GET".to_string(),
        headers,
        body: Vec::new(),
        correlation_id,
    };

    info!("Requesting S3 list: {}", req_task.url);

    crate::calls::network::on_http(&req_task);
}

// subs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_http_result(
    state: &mut State,
    response: veldsdk::proto::network::HttpTaskResponse,
) {
    let filter_path = state.pending_http.take(&response.correlation_id);
    
    let list_response = if response.status >= 200 && response.status < 300 {
        parse_s3_xml(response.body, filter_path.as_deref())
    } else {
        ListPathResponse {
            items: vec![],
            next_token: "".into(),
            error: format!("HTTP error: {}", response.status),
        }
    };

    crate::emit::on_list_path_result(&list_response);
}

pub fn on_fs_download_progress(
    state: &mut State,
    event: veldsdk::proto::network::FsDownloadProgress,
) {
    // Транслируем прогресс только своих активных загрузок.
    if !state.tasks.is_pending(&event.correlation_id) {
        return;
    }
    crate::emit::on_download_progress(&DownloadProgress {
        task_id: event.correlation_id,
        progress: event.progress,
    });
}

pub fn on_fs_download_result(
    state: &mut State,
    response: veldsdk::proto::network::FsDownloadResponse,
) {
    let correlation_id = response.correlation_id;
    if !state.tasks.finish(&correlation_id) {
        return;
    }

    let success = response.error.is_empty();
    crate::emit::on_downloaded(&Downloaded {
        task_id: correlation_id,
        success,
        error: response.error,
    });
}

// utils ---------------------------------------------------------------------------------------------------------------------------

fn get_s3_headers(state: &State, method: &str, uri: &str) -> std::collections::HashMap<String, String> {
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
