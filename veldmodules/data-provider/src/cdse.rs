//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{
    DataProduct, ImageryRaster, ImageryRequest, ImageryResponse, ImageryRole, ListEntry,
    ListPathRequest, ListPathResponse, LocateRequest, LocateResponse, ProductRoots,
    ProductRootsRequest, SearchRequest, SearchResponse, SignRequest, SignedUrl, UtmFrame,
};
use aws_smithy_runtime_api::client::identity::Identity;
use std::collections::HashMap;
use super::{catalogue, imagery, manifest, mgrs, s3, scene, Asked, Config, Pending, State};

/// Сейчас, unix-секунды. Нужно одному — нижней границе окна поиска
/// (см. `catalogue::FRESH_DAYS`).
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Спросить сеть и запомнить, чей это ход.
///
/// Единственное место, где заводится задача HTTP: ожидание должно появиться
/// раньше ответа, и вид запроса записан в нём же — каталог и хранилище отвечают
/// в один топик, и по ответу их не различить. Заголовки бывают только у запроса
/// к хранилищу, приходят они парой с адресом (см. `s3::Request`), и разойтись
/// им нельзя.
fn ask(
    state: &mut State,
    correlation_id: String,
    url: String,
    headers: HashMap<String, String>,
    what: Asked,
) {
    let internal_id = state.pending_http.begin(Pending { correlation_id, what });
    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url,
        method: "GET".to_string(),
        headers,
        body: Vec::new(),
    }, &internal_id);
}

/// Страница обхода поддерева продукта. Страниц бывает несколько, и все они —
/// одно ожидание: найденные ключи копятся в нём, пока хранилище не отдаст
/// последнюю (см. `Asked::Imagery`).
fn imagery_page(
    state: &mut State,
    correlation_id: String,
    identifier: String,
    keys: Vec<String>,
    token: &str,
) {
    let path = format!("{}/", identifier.trim_end_matches('/'));
    let listing = s3::listing_deep(&state.identity, &path, token);
    let what = Asked::Imagery { identifier, keys };
    ask(state, correlation_id, listing.url, listing.headers, what);
}

/// Ответ на `on_locate` — собирается он только здесь.
///
/// Кончиться `on_locate` может в пяти местах, и всюду ответ одной формы —
/// разнится в нём `answered`: «такого продукта нет» это ответ, а «спросить не
/// вышло» нет, и заказчик вправе прийти с тем же ключом снова (см.
/// `LocateResponse` в types.proto).
fn answer(correlation_id: &str, product: Option<DataProduct>, error: String, answered: bool) {
    crate::emit::on_locate_result(&LocateResponse { product, error, answered }, correlation_id);
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

// ── Входы ──────────────────────────────────────────────────────

/// Поиск по каталогу.
///
/// Подписи здесь нет и быть не должно: метаданные CDSE отдаёт всем, а ключи
/// нужны только хранилищу. Ответ уходит заказчику эхом по correlation_id — как
/// и у листинга, с которым поиск делит и путь через сеть, и топик ответа.
pub fn on_search(state: &mut State, request: SearchRequest) {
    let floor = now() - catalogue::FRESH_DAYS * 24 * 60 * 60;
    let url = catalogue::search(&request, floor);
    log::info!(target: "handlers", "Поиск в каталоге: {}", url);
    ask(state, veldsdk::correlation(), url, HashMap::new(),
        Asked::Search { request, widened: false });
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
            // Соседей у продукта-объекта нет по определению: в хранилище он
            // один ключ, и координатам лежать негде, кроме как в нём самом.
            Some((identifier, role)) => ImageryResponse {
                rasters: vec![raster(identifier, role, &[])],
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

    log::info!(target: "handlers",
        "Растры продукта: {}/", request.identifier.trim_end_matches('/'));
    imagery_page(state, veldsdk::correlation(), request.identifier, Vec::new(), "");
}

/// Растр в том виде, в каком его понимает контракт: роль здесь и роль в
/// `imagery` — два разных перечисления, и связаны они match'ем, а не
/// совпадением чисел.
fn raster(identifier: String, role: imagery::Role, keys: &[String]) -> ImageryRaster {
    ImageryRaster {
        // Координаты просит только измерительный растр: привязка у наложения
        // одна на все его растры, и приносит её он — квиклук ложится по ней же
        // (своей у него нет и быть не может, картинка о Земле не знает).
        geolocation: match role {
            imagery::Role::Detailed => imagery::geolocation(keys, &identifier).unwrap_or_default(),
            imagery::Role::Preview => String::new(),
        },
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
    let rasters =
        found.into_iter().map(|(key, role)| raster(key, role, keys)).collect::<Vec<_>>();

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
        answer(
            &veldsdk::correlation(),
            None,
            "пустой identifier: искать в каталоге нечего".to_string(),
            true,
        );
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
    ask(state, veldsdk::correlation(), url, HashMap::new(), Asked::Locate { name });
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
    log::info!(target: "handlers", "Листинг S3: {}", listing.url);

    ask(state, veldsdk::correlation(), listing.url, listing.headers, Asked::List(request.path));
}

// ── Ответы сети ────────────────────────────────────────────────

/// Разобрать ответ хранилища или сказать, почему разбирать нечего.
///
/// Об отказе хранилище говорит одним кодом состояния — тела с объяснением у
/// него нет, и код здесь всё, что можно показать.
fn from_storage<T>(
    response: &veldsdk::proto::network::HttpTaskResponse,
    parse: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> Result<T, String> {
    match (200..300).contains(&response.status) {
        true => parse(&response.body).map_err(|error| error.to_string()),
        false => Err(format!("хранилище ответило {}", response.status)),
    }
}

/// То же для каталога, с одной разницей: отказ он объясняет телом ответа, и
/// объяснение полезнее кода — «негодный фильтр» и «нет такой коллекции» с виду
/// одинаковы.
fn from_catalogue<T>(
    response: &veldsdk::proto::network::HttpTaskResponse,
    parse: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> Result<T, String> {
    match (200..300).contains(&response.status) {
        true => parse(&response.body).map_err(|error| error.to_string()),
        false => Err(catalogue::failure(&response.body)
            .unwrap_or_else(|| format!("каталог ответил {}", response.status))),
    }
}

/// Ответ сети. Чей он — листинга или поиска — записано в самом ожидании, и
/// разбирается он тем, кто его просил.
pub fn on_http_result(
    state: &mut State,
    response: veldsdk::proto::network::HttpTaskResponse,
) {
    let Some(pending) = state.pending_http.take(&veldsdk::correlation()) else {
        return;
    };

    match pending.what {
        Asked::List(path) => {
            let listing = from_storage(&response, |body| s3::parse_listing(body, &path));

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
            let listing = from_storage(&response, |body| s3::parse_listing(body, &identifier));

            match listing {
                Ok(listing) => {
                    keys.extend(listing.entries.into_iter().map(|entry| entry.identifier));
                    // Страница не последняя — тем же ожиданием за следующей;
                    // заказчику отвечать рано.
                    if !listing.next_token.is_empty() {
                        imagery_page(
                            state,
                            pending.correlation_id,
                            identifier,
                            keys,
                            &listing.next_token,
                        );
                        return;
                    }
                    // Обход кончился. Манифест спрашивается только там, где
                    // без него выбор был бы догадкой: у известных раскладок
                    // (Sentinel-1, Sentinel-2) подробный растр уже назван их
                    // собственным правилом, а манифест — это ещё один
                    // подписанный запрос на сотни килобайт, а у OLCI полного
                    // разрешения и на полтора мегабайта.
                    let known = imagery::scan(&keys, &[]);
                    if known.guessed
                        && let Some(manifest) = manifest::key(&identifier, &keys)
                    {
                        let object = s3::object(&state.identity, manifest);
                        let what = Asked::Manifest { identifier, keys };
                        ask(
                            state,
                            pending.correlation_id,
                            object.url,
                            object.headers,
                            what,
                        );
                        return;
                    }
                    crate::emit::on_imagery_result(
                        &imagery_response(&identifier, &keys, known.rasters),
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
        Asked::Manifest { identifier, keys } => {
            // Манифест не достался — это не отказ продукту: раскладка
            // разбирается по именам файлов, как и до манифеста. Сказать об
            // этом стоит: выбор растра тогда объясняется другим правилом.
            let measured = match (200..300).contains(&response.status) {
                true => manifest::measurements(&response.body),
                false => {
                    log::warn!(target: "handlers",
                        "Манифест '{}' не достался: хранилище ответило {}",
                        identifier, response.status);
                    Vec::new()
                }
            };
            if measured.is_empty() {
                log::debug!(target: "handlers",
                    "Манифест '{}' измерений не назвал — выбор по именам файлов", identifier);
            }
            crate::emit::on_imagery_result(
                &imagery_response(&identifier, &keys, imagery::scan(&keys, &measured).rasters),
                &pending.correlation_id,
            );
        }
        Asked::Search { request, widened } => {
            let (found, error) = match from_catalogue(&response, catalogue::scenes) {
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
            // Верхняя граница считается наравне с нижней: своей границы мы не
            // ставили и там, где заказчик назвал только «по» (см.
            // `catalogue::search`), — а повтор того же запроса ушёл бы в сеть
            // побайтно тем же и вернул бы то же самое.
            let ours = request.from <= 0 && request.to <= 0;
            if error.is_empty() && !widened && drained && products.len() < want && ours {
                let url = catalogue::search(&request, 0);
                log::info!(target: "handlers",
                    "В свежем окне всего {} продуктов — ищем за всё время", raw);
                ask(state, pending.correlation_id, url, HashMap::new(),
                    Asked::Search { request, widened: true });
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
            let found = match from_catalogue(&response, catalogue::parse) {
                Ok(found) => found,
                // А это не ответ вовсе — спросить не вышло.
                Err(error) => {
                    log::warn!(target: "handlers", "Продукт '{}' не нашёлся: {}", name, error);
                    answer(&pending.correlation_id, None, error, false);
                    return;
                }
            };

            let Some((facts, product)) = found.into_iter().next() else {
                // Пустой ответ — тоже ответ: ключ не из каталога (климатика,
                // вспомогательные данные) либо продукт из него уже ушёл.
                answer(
                    &pending.correlation_id,
                    None,
                    format!("в каталоге нет продукта с именем '{}'", name),
                    true,
                );
                return;
            };

            // Спросили об одной части, а показывать надо снимок — то есть ту из
            // частей, которая для этого годится. Соседей каталог знает, и это
            // ещё один ход к нему, а не ответ.
            // Спрашивать соседей стои́т только там, где каталог вообще связал
            // продукт с чем-то: не связал — соседей у него и не будет, а ход к
            // каталогу лишний.
            match scene::acquisition(&facts).is_some() {
                true => {
                    let url =
                        catalogue::siblings(&facts.platform, &facts.tile, product.acquired);
                    ask(state, pending.correlation_id, url, HashMap::new(),
                        Asked::Siblings { facts, found: product });
                }
                false => answer(&pending.correlation_id, Some(product), String::new(), true),
            }
        }
        Asked::Siblings { facts, found } => {
            // Соседние части — добавка, а не ответ: не нашлись или не
            // спросились — заказчик получает найденное как есть. Отказывать
            // из-за соседей значило бы потерять и сам продукт.
            //
            // Отбирает их то же правило, которым поиск сводит снимки
            // (`scene::same_scene`), а не голый ключ: у частей без номера
            // слайса ключи расходятся, и по голому ключу та же строка
            // раскрывается по-разному в зависимости от того, откуда о ней
            // спросили.
            let others = from_catalogue(&response, catalogue::parse).unwrap_or_default();
            let same = scene::same_scene(&facts, &found.name, others);
            let product = match scene::about(same, &found.identifier) {
                Some(scene) => {
                    if scene.identifier != found.identifier {
                        log::info!(target: "handlers", "'{}' показывается частью '{}'", found.name, scene.name);
                    }
                    scene
                }
                None => found,
            };
            answer(&pending.correlation_id, Some(product), String::new(), true);
        }
    }
}
