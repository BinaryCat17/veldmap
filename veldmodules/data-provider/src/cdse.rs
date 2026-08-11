//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{
    ListEntry, ListPathRequest, ListPathResponse, SearchRequest, SearchResponse, SignRequest,
    SignedUrl,
};
use aws_smithy_runtime_api::client::identity::Identity;
use super::{catalogue, s3, Asked, Config, Pending, State};

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

/// Поиск по каталогу.
///
/// Подписи здесь нет и быть не должно: метаданные CDSE отдаёт всем, а ключи
/// нужны только хранилищу. Ответ уходит заказчику эхом по correlation_id — как
/// и у листинга, с которым поиск делит и путь через сеть, и топик ответа.
pub fn on_search(state: &mut State, request: SearchRequest) {
    let url = catalogue::search(&request);
    let internal_id = state.pending_http.begin(Pending {
        correlation_id: veldsdk::correlation(),
        what: Asked::Search,
    });

    log::info!(target: "handlers", "Поиск в каталоге: {}", url);

    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url,
        method: "GET".to_string(),
        headers: Default::default(),
        body: Vec::new(),
    }, &internal_id);
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
    let internal_id = state.pending_http.begin(Pending {
        correlation_id: veldsdk::correlation(),
        what: Asked::List(request.path),
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

/// Ответ сети. Чей он — листинга или поиска — записано в самом ожидании, и
/// разбирается он тем, кто его просил.
pub fn on_http_result(
    state: &mut State,
    response: veldsdk::proto::network::HttpTaskResponse,
) {
    let Some(pending) = state.pending_http.take(&veldsdk::correlation()) else {
        return;
    };
    let ok = (200..300).contains(&response.status);

    match pending.what {
        Asked::List(path) => {
            let listing = if ok {
                s3::parse_listing(&response.body, &path).map_err(|error| error.to_string())
            } else {
                Err(format!("HTTP error: {}", response.status))
            };

            let (entries, next_token, error) = match listing {
                Ok(listing) => (
                    listing.entries.into_iter()
                        .map(|entry| ListEntry { key: entry.identifier, size: entry.size, modified: entry.modified })
                        .collect(),
                    listing.next_token,
                    String::new(),
                ),
                Err(error) => {
                    log::warn!(target: "handlers", "Листинг '{}' не удался: {}", path, error);
                    (Vec::new(), String::new(), error)
                }
            };

            crate::emit::on_list_path_result(&ListPathResponse {
                entries,
                next_token,
                error,
            }, &pending.correlation_id);
        }
        Asked::Search => {
            let found = if ok {
                catalogue::parse(&response.body).map_err(|error| error.to_string())
            } else {
                // Каталог объясняет отказ телом ответа, и объяснение полезнее
                // кода: «негодный фильтр» и «нет такой коллекции» с виду
                // одинаковы.
                Err(catalogue::failure(&response.body)
                    .unwrap_or_else(|| format!("каталог ответил {}", response.status)))
            };

            let (products, error) = match found {
                Ok(products) => {
                    log::info!(target: "handlers", "Найдено снимков: {}", products.len());
                    (products, String::new())
                }
                Err(error) => {
                    log::warn!(target: "handlers", "Поиск не удался: {}", error);
                    (Vec::new(), error)
                }
            };

            crate::emit::on_search_result(&SearchResponse { products, error }, &pending.correlation_id);
        }
    }
}
