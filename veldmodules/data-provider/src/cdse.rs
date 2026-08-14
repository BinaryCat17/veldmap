//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{
    ImageryRaster, ImageryRequest, ImageryResponse, ImageryRole, ListEntry, ListPathRequest,
    ListPathResponse, LocateRequest, LocateResponse, ProductRoots, ProductRootsRequest,
    SearchRequest, SearchResponse, SignRequest, SignedUrl, UtmFrame,
};
use aws_smithy_runtime_api::client::identity::Identity;
use super::{catalogue, imagery, mgrs, s3, Asked, Config, Pending, State};

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
    crate::emit::on_open_result(&veldsdk::resource::relay(&opened, &owner), &correlation_id);
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
    let signed = if req.identifier.is_empty() {
        SignedUrl {
            error: "пустой identifier: подписывать нечего".to_string(),
            ..Default::default()
        }
    } else {
        let object = s3::object(&state.identity, &req.identifier);
        SignedUrl { url: object.url, headers: object.headers, error: String::new() }
    };
    crate::emit::on_signed(&signed, &veldsdk::correlation());
}

/// Растры продукта для наложения. Поддерево листается целиком (без
/// delimiter): гранула — одна-две сотни ключей, это одна-две страницы, а
/// обход по уровням стоил бы больше запросов и знанием глубины раскладки.
pub fn on_imagery(state: &mut State, request: ImageryRequest) {
    if request.identifier.is_empty() {
        crate::emit::on_imagery_result(&ImageryResponse {
            error: "пустой identifier: искать растры негде".to_string(),
            ..Default::default()
        }, &veldsdk::correlation());
        return;
    }

    let path = format!("{}/", request.identifier.trim_end_matches('/'));
    let listing = s3::listing_deep(&state.identity, &path, "");
    let internal_id = state.pending_http.begin(Pending {
        correlation_id: veldsdk::correlation(),
        what: Asked::Imagery { identifier: request.identifier, keys: Vec::new() },
    });

    log::info!(target: "handlers", "Растры продукта: {}", path);

    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url: listing.url,
        method: "GET".to_string(),
        headers: listing.headers,
        body: Vec::new(),
    }, &internal_id);
}

/// Конец обхода поддерева: ключи → роли растров и рамка тайла из имени.
fn imagery_response(identifier: &str, keys: &[String]) -> ImageryResponse {
    let rasters = imagery::scan(keys)
        .into_iter()
        .map(|(identifier, role)| ImageryRaster {
            identifier,
            role: match role {
                imagery::Role::Preview => ImageryRole::ImageryPreview,
                imagery::Role::Detailed => ImageryRole::ImageryDetailed,
            } as i32,
        })
        .collect::<Vec<_>>();

    // Рамка — только когда её видно из имени (тайл Sentinel-2). Ошибка разбора
    // не валит ответ: без рамки потребитель живёт на футпринте каталога.
    let name = identifier.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let utm = mgrs::tile_of(name).and_then(|tile| match mgrs::frame(tile) {
        Ok(frame) => Some(UtmFrame {
            zone: frame.zone,
            south: frame.south,
            x0: frame.x0,
            y0: frame.y0,
            x1: frame.x1,
            y1: frame.y1,
        }),
        Err(error) => {
            log::warn!(target: "handlers", "Рамка тайла '{}': {}", tile, error);
            None
        }
    });

    if rasters.is_empty() {
        log::info!(target: "handlers", "У '{}' нет растров для наложения", name);
    }
    ImageryResponse { rasters, utm, error: String::new() }
}

/// Продукт каталога по ключу хранилища. Подъём к корню — знание раскладки
/// бакета (s3::product_root), дальше обычный запрос к каталогу, только по
/// точному имени и за одним продуктом.
/// Корни снимков для пачки ключей — ответ, для которого никуда идти не надо:
/// границу снимка задаёт раскладка бакета, и видна она в самом ключе.
///
/// Ключи, не лежащие ни в каком снимке, в ответ не попадают вовсе
/// (см. `ProductRoots.roots`).
pub fn on_product_roots(_state: &mut State, request: ProductRootsRequest) {
    let roots = request
        .identifiers
        .iter()
        .filter_map(|identifier| {
            let root = crate::module::s3::product_root(identifier)?;
            Some((identifier.clone(), root.to_string()))
        })
        .collect();
    crate::emit::on_product_roots_result(&ProductRoots { roots }, &veldsdk::correlation());
}

pub fn on_locate(state: &mut State, request: LocateRequest) {
    if request.identifier.is_empty() {
        crate::emit::on_locate_result(&LocateResponse {
            error: "пустой identifier: искать в каталоге нечего".to_string(),
            ..Default::default()
        }, &veldsdk::correlation());
        return;
    }

    // Корня нет — ключ лежит в пути к снимкам, а не в снимке; спрашиваем каталог
    // по нему самому: имя одиночного объекта каталог знает, а имени у папки года
    // нет, и ответом будет честное «не нашлось».
    let trimmed = request.identifier.trim_end_matches('/');
    let root = s3::product_root(trimmed).unwrap_or(trimmed);
    let name = root.rsplit('/').next().unwrap_or(root).to_string();
    let url = catalogue::locate(&name);
    let internal_id = state.pending_http.begin(Pending {
        correlation_id: veldsdk::correlation(),
        what: Asked::Locate { name },
    });

    log::info!(target: "handlers", "Продукт по имени: {}", url);

    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url,
        method: "GET".to_string(),
        headers: Default::default(),
        body: Vec::new(),
    }, &internal_id);
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

    log::info!(target: "handlers", "Листинг S3: {}", listing.url);

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
                Err(format!("хранилище ответило {}", response.status))
            };

            let (entries, next_token, error) = match listing {
                Ok(listing) => (
                    listing.entries.into_iter()
                        .map(|entry| ListEntry {
                            product: s3::product_root(&entry.identifier)
                                .unwrap_or_default()
                                .to_string(),
                            key: entry.identifier,
                            size: entry.size,
                            modified: entry.modified,
                        })
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
        Asked::Imagery { identifier, mut keys } => {
            let listing = if ok {
                s3::parse_listing(&response.body, &identifier).map_err(|error| error.to_string())
            } else {
                Err(format!("хранилище ответило {}", response.status))
            };

            match listing {
                Ok(listing) => {
                    keys.extend(listing.entries.into_iter().map(|entry| entry.identifier));
                    // Страница не последняя — тем же ожиданием за следующей;
                    // заказчику отвечать рано.
                    if !listing.next_token.is_empty() {
                        let path = format!("{}/", identifier.trim_end_matches('/'));
                        let next = s3::listing_deep(&state.identity, &path, &listing.next_token);
                        let internal_id = state.pending_http.begin(Pending {
                            correlation_id: pending.correlation_id,
                            what: Asked::Imagery { identifier, keys },
                        });
                        crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
                            url: next.url,
                            method: "GET".to_string(),
                            headers: next.headers,
                            body: Vec::new(),
                        }, &internal_id);
                        return;
                    }
                    crate::emit::on_imagery_result(
                        &imagery_response(&identifier, &keys),
                        &pending.correlation_id,
                    );
                }
                Err(error) => {
                    log::warn!(target: "handlers", "Растры '{}' не нашлись: {}", identifier, error);
                    crate::emit::on_imagery_result(&ImageryResponse {
                        error,
                        ..Default::default()
                    }, &pending.correlation_id);
                }
            }
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
        Asked::Locate { name } => {
            let found = if ok {
                catalogue::parse(&response.body).map_err(|error| error.to_string())
            } else {
                Err(catalogue::failure(&response.body)
                    .unwrap_or_else(|| format!("каталог ответил {}", response.status)))
            };

            let response = match found.map(|products| products.into_iter().next()) {
                Ok(Some(product)) => LocateResponse { product: Some(product), error: String::new() },
                // Пустой ответ — тоже ответ: ключ не из каталога (климатика,
                // вспомогательные данные) либо продукт из него уже ушёл.
                Ok(None) => LocateResponse {
                    product: None,
                    error: format!("в каталоге нет продукта с именем '{}'", name),
                },
                Err(error) => {
                    log::warn!(target: "handlers", "Продукт '{}' не нашёлся: {}", name, error);
                    LocateResponse { product: None, error }
                }
            };
            crate::emit::on_locate_result(&response, &pending.correlation_id);
        }
    }
}
