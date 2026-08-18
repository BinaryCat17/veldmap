//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{
    ImageryRaster, ImageryRequest, ImageryResponse, ImageryRole, ListEntry, ListPathRequest,
    ListPathResponse, LocateRequest, LocateResponse, ProductRoots, ProductRootsRequest,
    SearchRequest, SearchResponse, SignRequest, SignedUrl, UtmFrame,
};
use aws_smithy_runtime_api::client::identity::Identity;
use super::{catalogue, imagery, mgrs, s3, scene, Asked, Config, Pending, State};

/// Сейчас, unix-секунды. Нужно одному — нижней границе окна поиска
/// (см. `catalogue::FRESH_DAYS`).
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Спросить каталог и запомнить, чей это ход.
fn ask(state: &mut State, correlation_id: String, url: String, what: Asked) {
    let internal_id = state.pending_http.begin(Pending { correlation_id, what });
    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url,
        method: "GET".to_string(),
        headers: Default::default(),
        body: Vec::new(),
    }, &internal_id);
}

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
    let floor = now() - catalogue::FRESH_DAYS * 24 * 60 * 60;
    let url = catalogue::search(&request, floor);
    log::info!(target: "handlers", "Поиск в каталоге: {}", url);
    ask(state, veldsdk::correlation(), url, Asked::Search { request, widened: false });
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

    // Продукт-объект листать нечем: под его путём в хранилище нет ключей, и
    // пустой листинг сказал бы «ничего нет» там, где на самом деле «это файл».
    // Растром такой продукт бывает и сам — тогда он же и единственный растр.
    if s3::is_single_object(&request.identifier) {
        let response = match imagery::single(&request.identifier) {
            Some((identifier, role)) => ImageryResponse {
                rasters: vec![raster(identifier, role)],
                ..Default::default()
            },
            None => ImageryResponse {
                reason: imagery::just_a_file(&request.identifier),
                ..Default::default()
            },
        };
        crate::emit::on_imagery_result(&response, &veldsdk::correlation());
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

/// Растр в том виде, в каком его понимает контракт: роль здесь и роль в
/// `imagery` — два разных перечисления, и связаны они match'ем, а не
/// совпадением чисел.
fn raster(identifier: String, role: imagery::Role) -> ImageryRaster {
    ImageryRaster {
        identifier,
        role: match role {
            imagery::Role::Preview => ImageryRole::ImageryPreview,
            imagery::Role::Detailed => ImageryRole::ImageryDetailed,
        } as i32,
    }
}

/// Конец обхода поддерева: роли растров, рамка тайла из имени и объяснение,
/// если растров не нашлось.
///
/// `keys` — ключи самого продукта: ими объясняется пустой ответ, и ключи
/// зеркала для этого не годятся — спрашивали не о нём.
fn imagery_response(
    identifier: &str,
    keys: &[String],
    found: Vec<(String, imagery::Role)>,
) -> ImageryResponse {
    let empty = found.is_empty();
    let rasters = found.into_iter().map(|(key, role)| raster(key, role)).collect::<Vec<_>>();

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

    let reason = match empty {
        true => imagery::nothing_here(identifier, keys),
        false => String::new(),
    };
    if empty {
        log::info!(target: "handlers", "У '{}' наложению нечего показать: {}", name, reason);
    }
    ImageryResponse { rasters, utm, error: String::new(), reason }
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
        // Отвечено, и повторять нечего: пустой ключ пустым и останется.
        crate::emit::on_locate_result(&LocateResponse {
            error: "пустой identifier: искать в каталоге нечего".to_string(),
            answered: true,
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
    log::info!(target: "handlers", "Продукт по имени: {}", url);
    ask(state, veldsdk::correlation(), url, Asked::Locate { name });
}

pub fn on_list_path(
    state: &mut State,
    request: ListPathRequest
) {
    // Разделитель — это и есть вся разница между уровнем и поддеревом: с ним
    // хранилище отвечает папками как общими префиксами, без него разворачивает
    // все ключи под путём.
    let listing = match request.recursive {
        true => s3::listing_deep(&state.identity, &request.path, &request.token),
        false => s3::listing(&state.identity, &request.path, &request.token),
    };
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
                        .map(|entry| {
                            let product = s3::product_root(&entry.identifier)
                                .unwrap_or_default()
                                .to_string();
                            // На шар кладут снимок; уровня обработки листинг не
                            // знает — его сообщает только каталог.
                            let itself = product == entry.identifier.trim_end_matches('/');
                            let viewable = itself
                                && imagery::showable(
                                    &entry.identifier,
                                    entry.identifier.ends_with('/'),
                                    None,
                                );
                            ListEntry {
                                product,
                                key: entry.identifier,
                                size: entry.size,
                                modified: entry.modified,
                                viewable,
                            }
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
                        &imagery_response(&identifier, &keys, imagery::scan(&keys)),
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
        Asked::Search { request, widened } => {
            let found = if ok {
                catalogue::scenes(&response.body).map_err(|error| error.to_string())
            } else {
                // Каталог объясняет отказ телом ответа, и объяснение полезнее
                // кода: «негодный фильтр» и «нет такой коллекции» с виду
                // одинаковы.
                Err(catalogue::failure(&response.body)
                    .unwrap_or_else(|| format!("каталог ответил {}", response.status)))
            };

            let (found, error) = match found {
                Ok(found) => (found, String::new()),
                Err(error) => {
                    log::warn!(target: "handlers", "Поиск не удался: {}", error);
                    (catalogue::Found { scenes: Vec::new(), products: 0 }, error)
                }
            };
            let catalogue::Found { scenes: mut products, products: raw } = found;

            // Окно свежести кончилось, а страница не набралась — значит границу
            // поставили мы, и снять её должны тоже мы. Кончилось оно тогда,
            // когда каталог отдал меньше продуктов, чем спрошено: полная
            // страница продуктов при короткой странице снимков означает не
            // пустое окно, а сведение — там есть ещё, надо просто листать
            // дальше. Второй раз окно уже не ставится: медленный ответ
            // каталога здесь оплачен делом (см. `catalogue::FRESH_DAYS`).
            let want = catalogue::wanted(&request) as usize;
            let drained = raw < catalogue::asked(&request) as usize;
            if error.is_empty() && !widened && drained && products.len() < want && request.from <= 0
            {
                let url = catalogue::search(&request, 0);
                log::info!(target: "handlers",
                    "В свежем окне всего {} продуктов — ищем за всё время", raw);
                ask(state, pending.correlation_id, url, Asked::Search { request, widened: true });
                return;
            }

            products.truncate(want);
            if error.is_empty() {
                log::info!(target: "handlers",
                    "Найдено снимков: {} (продуктов в ответе каталога: {})", products.len(), raw);
            }
            crate::emit::on_search_result(&SearchResponse { products, error }, &pending.correlation_id);
        }
        Asked::Locate { name } => {
            let found = if ok {
                catalogue::parse(&response.body).map_err(|error| error.to_string())
            } else {
                Err(catalogue::failure(&response.body)
                    .unwrap_or_else(|| format!("каталог ответил {}", response.status)))
            };

            let found = match found {
                Ok(found) => found,
                // А это не ответ вовсе — спросить не вышло.
                Err(error) => {
                    log::warn!(target: "handlers", "Продукт '{}' не нашёлся: {}", name, error);
                    crate::emit::on_locate_result(
                        &LocateResponse { product: None, error, answered: false },
                        &pending.correlation_id,
                    );
                    return;
                }
            };

            let Some((facts, product)) = found.into_iter().next() else {
                // Пустой ответ — тоже ответ: ключ не из каталога (климатика,
                // вспомогательные данные) либо продукт из него уже ушёл.
                crate::emit::on_locate_result(&LocateResponse {
                    product: None,
                    error: format!("в каталоге нет продукта с именем '{}'", name),
                    answered: true,
                }, &pending.correlation_id);
                return;
            };

            // Спросили об одной части, а показывать надо снимок — то есть ту из
            // частей, которая для этого годится. Соседей каталог знает, и это
            // ещё один ход к нему, а не ответ.
            match scene::acquisition(&facts) {
                Some(key) => {
                    let url = catalogue::siblings(&facts.platform, product.acquired);
                    ask(state, pending.correlation_id, url, Asked::Siblings { key, found: product });
                }
                None => crate::emit::on_locate_result(&LocateResponse {
                    product: Some(product),
                    error: String::new(),
                    answered: true,
                }, &pending.correlation_id),
            }
        }
        Asked::Siblings { key, found } => {
            // Соседние части — добавка, а не ответ: не нашлись или не
            // спросились — заказчик получает найденное как есть. Отказывать
            // из-за соседей значило бы потерять и сам продукт.
            let others = match ok {
                true => catalogue::parse(&response.body).unwrap_or_default(),
                false => Vec::new(),
            };
            let same: Vec<_> = others
                .into_iter()
                .filter(|(facts, _)| scene::acquisition(facts).as_deref() == Some(key.as_str()))
                .collect();
            let product = match scene::about(same, &found.identifier) {
                Some(scene) => {
                    if scene.identifier != found.identifier {
                        log::info!(target: "handlers", "'{}' показывается частью '{}'", found.name, scene.name);
                    }
                    scene
                }
                None => found,
            };
            crate::emit::on_locate_result(&LocateResponse {
                product: Some(product),
                error: String::new(),
                answered: true,
            }, &pending.correlation_id);
        }
    }
}
