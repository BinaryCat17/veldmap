//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{ListPathRequest, ListPathResponse, SignRequest, SignedUrl};
use aws_smithy_runtime_api::client::identity::Identity;
use super::{s3, Config, PendingList, State};

pub fn module_init(config: Config) -> anyhow::Result<State> {
    let credentials = aws_credential_types::Credentials::new(
        config.access_key, 
        config.secret_key, 
        None, None, "veldmap"
    );
    let identity = Identity::new(credentials, None);
    
    Ok(State {
        identity,
        pending_http: veldsdk::Correlator::new(),
        opening: veldsdk::Correlator::new(),
    })
}

/// Открыть продукт как ресурс, не скачивая его.
///
/// Подписать запрос к S3 может только этот модуль (ключи у него), поэтому
/// открывает он — а владение готовым ресурсом сразу передаёт заказчику:
/// дальше тот читает его как обычный файл и сам решает, когда закрыть.
pub fn on_open(state: &mut State, request: crate::proto::data_provider::OpenRequest) {
    // Корреляция заказчика проходит насквозь: тем же id мы спрашиваем network
    // и тем же отвечаем ему — второго учёта эта передача не требует.
    let correlation_id = veldsdk::correlation();
    let owner = match veldsdk::resource::requester("data-provider/on_open") {
        Ok(owner) => owner,
        Err(e) => {
            crate::emit::on_open_result(&veldsdk::resource::opened(Err(e)), &correlation_id);
            return;
        }
    };

    let object = s3::object(&state.identity, &request.identifier);
    state.opening.insert(correlation_id.clone(), owner);

    crate::calls::network::on_open(&veldsdk::proto::network::RemoteOpenRequest {
        url: object.url,
        headers: object.headers,
    }, &correlation_id);
}

/// network открыл удалённый ресурс — передаём владение заказчику.
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    let correlation_id = veldsdk::correlation();
    let Some(owner) = state.opening.take(&correlation_id) else { return };
    let result = veldsdk::resource::accept(&opened)
        .and_then(|handle| veldsdk::resource::hand_off(handle, &owner));
    crate::emit::on_open_result(&veldsdk::resource::opened(result), &correlation_id);
}

//  inputs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_search(
    _state: &mut State, 
    _request: crate::proto::data_provider::SearchRequest
) {
    // TODO: Implement search via OData/OpenSearch
    log::info!(target: "handlers", "Search requested (not implemented)");
}

/// Подписать адрес продукта — единственное, чего заказчик не может сам.
///
/// Ответ уходит ему же, эхом по correlation_id. Ни задачи, ни учёта здесь
/// нет: закачку ведёт тот, кто её попросил, он же её владелец у платформы
/// и он же её отменяет.
pub fn on_sign(state: &mut State, req: SignRequest) {
    let error = if req.identifier.is_empty() {
        "пустой identifier: подписывать нечего".to_string()
    } else {
        String::new()
    };

    let object = (!req.identifier.is_empty()).then(|| s3::object(&state.identity, &req.identifier));
    let (url, headers) = match object {
        Some(o) => (o.url, o.headers),
        None => (String::new(), Default::default()),
    };

    crate::emit::on_signed(&SignedUrl { url, headers, error }, &veldsdk::correlation());
}

pub fn on_list_path(
    state: &mut State,
    request: ListPathRequest
) {
    let listing = s3::listing(&state.identity, &request.path, &request.token);
    let internal_id = state.pending_http.begin(PendingList {
        path: request.path,
        correlation_id: veldsdk::correlation(),
    });

    log::info!(target: "handlers", "Requesting S3 list: {}", listing.url);

    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url: listing.url,
        method: "GET".to_string(),
        headers: listing.headers,
        body: Vec::new(),
    }, &internal_id);
}

// subs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_http_result(
    state: &mut State,
    response: veldsdk::proto::network::HttpTaskResponse,
) {
    let Some(pending) = state.pending_http.take(&veldsdk::correlation()) else {
        return;
    };

    let listing = if response.status >= 200 && response.status < 300 {
        s3::parse_listing(&response.body, &pending.path).map_err(|e| e.to_string())
    } else {
        Err(format!("HTTP error: {}", response.status))
    };

    let (items, next_token, error) = match listing {
        Ok(listing) => (listing.items, listing.next_token, String::new()),
        Err(error) => {
            log::warn!(target: "handlers", "Листинг '{}' не удался: {}", pending.path, error);
            (Vec::new(), String::new(), error)
        }
    };

    crate::emit::on_list_path_result(&ListPathResponse {
        items,
        next_token,
        error,
    }, &pending.correlation_id);
}
