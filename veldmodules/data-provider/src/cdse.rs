use crate::proto::data_provider::{
    DataProduct, ImageryRaster, ImageryRequest, ImageryResponse, ImageryRole, ListEntry,
    ListPathRequest, ListPathResponse, LocateRequest, LocateResponse, ProductRoots,
    ProductRootsRequest, SearchRequest, SearchResponse, SignRequest, SignedUrl, UtmFrame,
};
use aws_smithy_runtime_api::client::identity::Identity;
use std::collections::HashMap;
use super::{catalogue, imagery, manifest, mgrs, s3, scene, Asked, Config, Pending, State};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

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

fn imagery_page(
    state: &mut State,
    correlation_id: String,
    identifier: String,
    keys: Vec<String>,
    downloaded: bool,
    wanted: String,
    token: &str,
) -> Result<(), String> {
    let path = format!("{}/", identifier.trim_end_matches('/'));
    let listing = {
        let Some(identity) = state.identity.as_ref() else { return Err(NO_KEYS.to_string()) };
        s3::listing_deep(identity, &path, token)
    };
    let what = Asked::Imagery { identifier, keys, downloaded, wanted };
    ask(state, correlation_id, listing.url, listing.headers, what);
    Ok(())
}

fn refuse_imagery(correlation_id: &str, error: String) {
    crate::emit::on_imagery_result(
        &ImageryResponse { error, ..Default::default() },
        correlation_id,
    );
}

fn answer(correlation_id: &str, product: Option<DataProduct>, error: String, answered: bool) {
    crate::emit::on_locate_result(&LocateResponse { product, error, answered }, correlation_id);
}

pub const NO_KEYS: &str = "no Copernicus keys: copy .env.example to \
    .env and fill in COPERNICUS_ACCESS_KEY and COPERNICUS_ACCESS_SECRET";

pub fn module_init(config: Config) -> anyhow::Result<State> {
    let access = config.access_key.trim();
    let secret = config.secret_key.trim();
    let identity = (!access.is_empty() && !secret.is_empty()).then(|| {
        let credentials = aws_credential_types::Credentials::new(
            access.to_string(),
            secret.to_string(),
            None, None, "veldmap"
        );
        Identity::new(credentials, None)
    });
    if identity.is_none() {
        log::warn!(target: "handlers", "{}", NO_KEYS);
    }

    Ok(State {
        identity,
        pending_http: veldsdk::Correlator::new(),
        opening: veldsdk::Correlator::new(),
    })
}

pub fn on_open(state: &mut State, request: crate::proto::data_provider::OpenRequest) {
    let correlation_id = veldsdk::correlation();
    let Some(identity) = state.identity.as_ref() else {
        crate::emit::on_open_result(
            &veldsdk::resource::opened(Err(NO_KEYS.to_string())),
            &correlation_id,
        );
        return;
    };
    let object = s3::object(identity, &request.identifier);

    let owner = match veldsdk::resource::requester("data-provider/on_open") {
        Ok(owner) => owner,
        Err(e) => {
            crate::emit::on_open_result(&veldsdk::resource::opened(Err(e)), &correlation_id);
            return;
        }
    };
    state.opening.insert(correlation_id.clone(), owner);

    crate::calls::network::on_open(&veldsdk::proto::network::RemoteOpenRequest {
        url: object.url,
        headers: object.headers,
    }, &correlation_id);
}

pub fn on_resign(state: &mut State, request: veldsdk::proto::network::ResignRequest) {
    let headers = state
        .identity
        .as_ref()
        .and_then(|identity| s3::resign(identity, &request.url))
        .map(|signed| signed.headers)
        .unwrap_or_default();
    if headers.is_empty() {
        log::warn!(target: "handlers", "{}: nothing to re-sign with", request.url);
    }
    crate::calls::network::on_resigned(&veldsdk::proto::network::ResignResponse { url: request.url, headers });
}

pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    let correlation_id = veldsdk::correlation();
    let Some(owner) = state.opening.take(&correlation_id) else { return };
    crate::emit::on_open_result(&veldsdk::resource::relay(&opened, &owner), &correlation_id);
}

pub fn on_search(state: &mut State, request: SearchRequest) {
    let floor = now() - catalogue::FRESH_DAYS * 24 * 60 * 60;
    let url = catalogue::search(&request, floor);
    log::info!(target: "handlers", "Catalogue search: {}", url);
    ask(state, veldsdk::correlation(), url, HashMap::new(),
        Asked::Search { request, widened: false });
}

pub fn on_sign(state: &mut State, req: SignRequest) {
    let signed = match (req.identifier.is_empty(), state.identity.as_ref()) {
        (true, _) => SignedUrl {
            error: "empty identifier: nothing to sign".to_string(),
            ..Default::default()
        },
        (false, None) => SignedUrl { error: NO_KEYS.to_string(), ..Default::default() },
        (false, Some(identity)) => {
            let object = s3::object(identity, &req.identifier);
            SignedUrl { url: object.url, headers: object.headers, error: String::new() }
        }
    };
    crate::emit::on_signed(&signed, &veldsdk::correlation());
}

pub fn on_imagery(state: &mut State, request: ImageryRequest) {
    if request.identifier.is_empty() {
        crate::emit::on_imagery_result(&ImageryResponse {
            error: "empty identifier: nowhere to look for rasters".to_string(),
            ..Default::default()
        }, &veldsdk::correlation());
        return;
    }

    if s3::is_single_object(&request.identifier) {
        if !request.wanted.is_empty() {
            log::info!(target: "handlers", "single-object product {} — a named file is pointless, it is the raster itself", request.identifier);
        }
        let response = match imagery::single(&request.identifier) {
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
        "Product rasters: {}/", request.identifier.trim_end_matches('/'));
    let correlation_id = veldsdk::correlation();
    if let Err(error) = imagery_page(
        state,
        correlation_id.clone(),
        request.identifier,
        Vec::new(),
        request.downloaded,
        request.wanted,
        "",
    ) {
        refuse_imagery(&correlation_id, error);
    }
}

fn raster(identifier: String, role: imagery::Role, keys: &[String]) -> ImageryRaster {
    ImageryRaster {
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

fn imagery_response(identifier: &str, keys: &[String], found: imagery::Scan) -> ImageryResponse {
    let empty = found.rasters.is_empty();
    let rasters = found
        .rasters
        .into_iter()
        .chain(found.alternates.into_iter().map(|key| (key, imagery::Role::Detailed)))
        .map(|(key, role)| raster(key, role, keys))
        .collect::<Vec<_>>();

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
            log::warn!(target: "handlers", "Tile frame '{}': {}", tile, error);
            None
        }
    });

    let reason = match empty {
        true => imagery::nothing_here(identifier, keys),
        false => String::new(),
    };
    if empty {
        log::info!(target: "handlers", "Nothing for the overlay to show at '{}': {}", name, reason);
    }
    ImageryResponse { rasters, utm, error: String::new(), reason, wanted_refused: String::new() }
}

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
        answer(
            &veldsdk::correlation(),
            None,
            "empty identifier: nothing to look up in the catalogue".to_string(),
            true,
        );
        return;
    }

    let trimmed = request.identifier.trim_end_matches('/');
    let root = s3::product_root(trimmed).unwrap_or(trimmed);
    let name = root.rsplit('/').next().unwrap_or(root).to_string();
    let url = catalogue::locate(&name);
    log::info!(target: "handlers", "Product by name: {}", url);
    ask(state, veldsdk::correlation(), url, HashMap::new(), Asked::Locate { name });
}

pub fn on_list_path(
    state: &mut State,
    request: ListPathRequest
) {
    let Some(identity) = state.identity.as_ref() else {
        crate::emit::on_list_path_result(&ListPathResponse {
            error: NO_KEYS.to_string(),
            ..Default::default()
        }, &veldsdk::correlation());
        return;
    };
    let listing = match request.recursive {
        true => s3::listing_deep(identity, &request.path, &request.token),
        false => s3::listing(identity, &request.path, &request.token),
    };
    log::info!(target: "handlers", "S3 listing: {}", listing.url);

    ask(state, veldsdk::correlation(), listing.url, listing.headers, Asked::List(request.path));
}

fn from_storage<T>(
    response: &veldsdk::proto::network::HttpTaskResponse,
    parse: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> Result<T, String> {
    match (200..300).contains(&response.status) {
        true => parse(&response.body).map_err(|error| error.to_string()),
        false => Err(format!("storage answered {}", response.status)),
    }
}

fn from_catalogue<T>(
    response: &veldsdk::proto::network::HttpTaskResponse,
    parse: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> Result<T, String> {
    match (200..300).contains(&response.status) {
        true => parse(&response.body).map_err(|error| error.to_string()),
        false => Err(catalogue::failure(&response.body)
            .unwrap_or_else(|| format!("catalogue answered {}", response.status))),
    }
}

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
                            let itself = product == entry.identifier.trim_end_matches('/');
                            let unviewable = match itself {
                                true => imagery::unviewable(
                                    &entry.identifier,
                                    entry.identifier.ends_with('/'),
                                    None,
                                    "",
                                ),
                                false => "whole scenes go onto the globe, and this entry is not one"
                                    .to_string(),
                            };
                            ListEntry {
                                product,
                                key: entry.identifier,
                                size: entry.size,
                                modified: entry.modified,
                                unviewable,
                            }
                        })
                        .collect(),
                    listing.next_token,
                    String::new(),
                ),
                Err(error) => {
                    log::warn!(target: "handlers", "Listing '{}' failed: {}", path, error);
                    (Vec::new(), String::new(), error)
                }
            };

            crate::emit::on_list_path_result(&ListPathResponse {
                entries,
                next_token,
                error,
            }, &pending.correlation_id);
        }
        Asked::Imagery { identifier, mut keys, downloaded, wanted } => {
            let listing = from_storage(&response, |body| s3::parse_listing(body, &identifier));

            match listing {
                Ok(listing) => {
                    keys.extend(listing.entries.into_iter().map(|entry| entry.identifier));
                    if !listing.next_token.is_empty() {
                        if let Err(error) = imagery_page(
                            state,
                            pending.correlation_id.clone(),
                            identifier,
                            keys,
                            downloaded,
                            wanted,
                            &listing.next_token,
                        ) {
                            refuse_imagery(&pending.correlation_id, error);
                        }
                        return;
                    }
                    let known = imagery::scan(&identifier, &keys, &[], downloaded);
                    if known.guessed
                        && let Some(manifest) = manifest::key(&identifier, &keys)
                        && let Some(identity) = state.identity.as_ref()
                    {
                        let object = s3::object(identity, manifest);
                        let what = Asked::Manifest { identifier, keys, downloaded, wanted };
                        ask(
                            state,
                            pending.correlation_id,
                            object.url,
                            object.headers,
                            what,
                        );
                        return;
                    }
                    let (found, wanted_refused) = imagery::preferring(known, &wanted, &keys);
                    crate::emit::on_imagery_result(
                        &ImageryResponse {
                            wanted_refused: wanted_refused.unwrap_or_default(),
                            ..imagery_response(&identifier, &keys, found)
                        },
                        &pending.correlation_id,
                    );
                }
                Err(error) => {
                    log::warn!(target: "handlers", "Rasters for '{}' not found: {}", identifier, error);
                    refuse_imagery(&pending.correlation_id, error);
                }
            }
        }
        Asked::Manifest { identifier, keys, downloaded, wanted } => {
            let measured = match (200..300).contains(&response.status) {
                true => manifest::measurements(&response.body),
                false => {
                    log::warn!(target: "handlers",
                        "Manifest '{}' was not fetched: storage answered {}",
                        identifier, response.status);
                    Vec::new()
                }
            };
            if measured.is_empty() {
                log::debug!(target: "handlers",
                    "Manifest '{}' named no measurements — choosing by file names", identifier);
            }
            let (found, wanted_refused) =
                imagery::preferring(imagery::scan(&identifier, &keys, &measured, downloaded), &wanted, &keys);
            crate::emit::on_imagery_result(
                &ImageryResponse {
                    wanted_refused: wanted_refused.unwrap_or_default(),
                    ..imagery_response(&identifier, &keys, found)
                },
                &pending.correlation_id,
            );
        }
        Asked::Search { request, widened } => {
            let (found, error) = match from_catalogue(&response, catalogue::scenes) {
                Ok(found) => (found, String::new()),
                Err(error) => {
                    log::warn!(target: "handlers", "Search failed: {}", error);
                    (catalogue::Found { scenes: Vec::new(), products: 0 }, error)
                }
            };
            let catalogue::Found { scenes: mut products, products: raw } = found;

            let want = catalogue::wanted(&request) as usize;
            let drained = raw < catalogue::asked(&request) as usize;
            let ours = request.from <= 0 && request.to <= 0;
            if error.is_empty() && !widened && drained && products.len() < want && ours {
                let url = catalogue::search(&request, 0);
                log::info!(target: "handlers",
                    "Only {} products in the fresh window — searching all of time", raw);
                ask(state, pending.correlation_id, url, HashMap::new(),
                    Asked::Search { request, widened: true });
                return;
            }

            products.truncate(want);
            if error.is_empty() {
                log::info!(target: "handlers",
                    "Scenes found: {} (products in the catalogue answer: {})", products.len(), raw);
            }
            crate::emit::on_search_result(&SearchResponse { products, error }, &pending.correlation_id);
        }
        Asked::Locate { name } => {
            let found = match from_catalogue(&response, catalogue::parse) {
                Ok(found) => found,
                Err(error) => {
                    log::warn!(target: "handlers", "Product '{}' not found: {}", name, error);
                    answer(&pending.correlation_id, None, error, false);
                    return;
                }
            };

            let Some((facts, product)) = found.into_iter().next() else {
                answer(
                    &pending.correlation_id,
                    None,
                    format!("no product named '{}' in the catalogue", name),
                    true,
                );
                return;
            };

            match scene::acquisition(&facts).is_some() {
                true => {
                    let url = catalogue::siblings(
                        &facts.platform,
                        &facts.tile,
                        &facts.datatake,
                        product.acquired,
                    );
                    ask(state, pending.correlation_id, url, HashMap::new(),
                        Asked::Siblings { facts, found: product });
                }
                false => answer(&pending.correlation_id, Some(product), String::new(), true),
            }
        }
        Asked::Siblings { facts, found } => {
            let others = from_catalogue(&response, catalogue::parse).unwrap_or_default();
            if others.len() >= catalogue::siblings_top(&facts.datatake) as usize {
                log::warn!(target: "handlers",
                    "neighbors of '{}': {} arrived — the output ceiling; a part of the acquisition may be missing",
                    found.name, others.len());
            }
            let same = scene::same_scene(&facts, &found.name, others);
            let product = match scene::about(same, &found.identifier) {
                Some(scene) => {
                    if scene.identifier != found.identifier {
                        log::info!(target: "handlers", "'{}' is shown via part '{}'", found.name, scene.name);
                    }
                    scene
                }
                None => found,
            };
            answer(&pending.correlation_id, Some(product), String::new(), true);
        }
    }
}
