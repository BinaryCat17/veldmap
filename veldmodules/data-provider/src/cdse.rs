//! data-provider topic handlers: CDSE products as files and as resources.
//!
//! How the bucket is laid out is in s3.rs; here is only the lifecycle of
//! requests and tasks.

use crate::proto::data_provider::{
    DataProduct, ImageryRaster, ImageryRequest, ImageryResponse, ImageryRole, ListEntry,
    ListPathRequest, ListPathResponse, LocateRequest, LocateResponse, ProductRoots,
    ProductRootsRequest, SearchRequest, SearchResponse, SignRequest, SignedUrl, UtmFrame,
};
use aws_smithy_runtime_api::client::identity::Identity;
use std::collections::HashMap;
use super::{catalogue, imagery, manifest, mgrs, s3, scene, Asked, Config, Pending, State};

/// Now, in unix seconds. Needed by exactly one thing — the lower bound of the
/// search window (see `catalogue::FRESH_DAYS`).
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Ask the network and remember whose move this is.
///
/// The only place where an HTTP task is created: the wait must appear before
/// the response, and the request kind is recorded in it — the catalogue and
/// the storage answer on one topic and cannot be told apart by the answer.
/// Headers belong only to storage requests, they come paired with the address
/// (see `s3::Request`), and the two must not part.
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

/// A page of the product subtree traversal. There can be several pages, and
/// all of them are a single wait: found keys accumulate in it until the
/// storage serves the last one (see `Asked::Imagery`).
///
/// A refusal is returned, not answered here: the page is requested by two —
/// the entry and the continuation of the traversal — and each of them must
/// answer the requester, otherwise it is left waiting for a response that
/// will never come.
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

/// A refusal in rasters — one shape for every place it happens.
fn refuse_imagery(correlation_id: &str, error: String) {
    crate::emit::on_imagery_result(
        &ImageryResponse { error, ..Default::default() },
        correlation_id,
    );
}

/// The answer to `on_locate` — assembled only here.
///
/// `on_locate` can end in five places, and everywhere the answer has one
/// shape — only `answered` differs: "no such product" is an answer, while
/// "the query could not be made" is not, and the requester may come with the
/// same key again (see `LocateResponse` in types.proto).
fn answer(correlation_id: &str, product: Option<DataProduct>, error: String, answered: bool) {
    crate::emit::on_locate_result(&LocateResponse { product, error, answered }, correlation_id);
}

/// What every move to the storage answers when there are no keys.
///
/// With its own refusal, not an S3 code: a request signed with empty keys
/// goes out and comes back 403, and by it "no keys at all" cannot be told
/// from "wrong keys" or "someone else's object". The person then sees the
/// same word in three different troubles, and the first of them is cured by
/// a line in a file.
pub const NO_KEYS: &str = "no Copernicus keys: copy .env.example to \
    .env and fill in COPERNICUS_ACCESS_KEY and COPERNICUS_ACCESS_SECRET";

pub fn module_init(config: Config) -> anyhow::Result<State> {
    // Empty keys are not a load failure, and the module comes up without
    // them: the scene catalogue is open, and search and pointing at it work.
    // Only the storage refuses — listing, rasters, signing, opening — and
    // each such move says so in its own response.
    // Surrounding whitespace is trimmed, and not only for the check: a key of
    // a single space never becomes a signature, while a key with a trailing
    // space does become one — a bad one. S3 answers both troubles the same
    // way, with 403, so they can only be told apart here.
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

/// Open a product as a resource without downloading it.
///
/// Only this module can sign a request to S3 (it holds the keys), so it does
/// the opening — and hands ownership of the ready resource to the requester
/// right away: from there the requester reads it like a plain file and
/// decides itself when to close it.
pub fn on_open(state: &mut State, request: crate::proto::data_provider::OpenRequest) {
    // The requester's correlation passes straight through: with the same id
    // we ask network and answer it — this handover needs no second accounting.
    let correlation_id = veldsdk::correlation();
    // The keys are asked before the requester: there is nothing to sign
    // with, and there is no point in tracking an opening that will not
    // happen.
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

/// network asks for the object to be re-signed: the signature expired
/// mid-read. The answer is headers with a fresh signature or empty ones: no
/// keys or the address is not ours, and the read on that side refuses in its
/// own words.
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

/// network opened a remote resource — hand ownership to the requester.
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    let correlation_id = veldsdk::correlation();
    let Some(owner) = state.opening.take(&correlation_id) else { return };
    crate::emit::on_open_result(&veldsdk::resource::relay(&opened, &owner), &correlation_id);
}

// ── Inputs ─────────────────────────────────────────────────────

/// Search the catalogue.
///
/// There is no signature here and there must be none: CDSE serves metadata
/// to everyone, and keys are needed only by the storage. The answer goes to
/// the requester as an echo by correlation_id — as with the listing, with
/// which the search shares both the path through the network and the answer
/// topic.
pub fn on_search(state: &mut State, request: SearchRequest) {
    let floor = now() - catalogue::FRESH_DAYS * 24 * 60 * 60;
    let url = catalogue::search(&request, floor);
    log::info!(target: "handlers", "Catalogue search: {}", url);
    ask(state, veldsdk::correlation(), url, HashMap::new(),
        Asked::Search { request, widened: false });
}

/// Sign a product address — the one thing the requester cannot do itself.
///
/// The answer goes back to it, echoed by correlation_id. No task and no
/// accounting here: the download is driven by whoever asked for it, who is
/// also its owner on the platform and who cancels it.
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

/// Product rasters for the overlay. The subtree is listed whole (without
/// delimiter): a granule is a hundred or two keys, that is one or two pages,
/// while walking by levels would cost more requests and knowledge of the
/// layout depth.
pub fn on_imagery(state: &mut State, request: ImageryRequest) {
    if request.identifier.is_empty() {
        crate::emit::on_imagery_result(&ImageryResponse {
            error: "empty identifier: nowhere to look for rasters".to_string(),
            ..Default::default()
        }, &veldsdk::correlation());
        return;
    }

    // A single-object product cannot be listed: its path holds no keys in the
    // storage, and an empty listing would say "nothing here" where the truth
    // is "this is a file". Such a product can be a raster itself — then it is
    // also the only raster.
    if s3::is_single_object(&request.identifier) {
        if !request.wanted.is_empty() {
            log::info!(target: "handlers", "single-object product {} — a named file is pointless, it is the raster itself", request.identifier);
        }
        let response = match imagery::single(&request.identifier) {
            // A single-object product has no neighbors by definition: in the
            // storage it is one key, and the coordinates have nowhere to lie
            // but inside it itself.
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

/// A raster in the form the contract understands: the role here and the role
/// in `imagery` are two different enums, and they are linked by a match, not
/// by matching numbers.
fn raster(identifier: String, role: imagery::Role, keys: &[String]) -> ImageryRaster {
    ImageryRaster {
        // Only the measuring raster asks for coordinates: the overlay has one
        // binding for all of its rasters, and it is this one that brings it —
        // the quicklook is laid by that same binding (it has none of its own
        // and cannot have: the picture knows nothing of the Earth).
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

/// The end of the subtree traversal: raster roles, the tile frame from the
/// name, and an explanation if no rasters were found.
///
/// `keys` are the product's own keys: they explain an empty answer, and
/// mirror keys do not serve for this — the question was not about the mirror.
fn imagery_response(identifier: &str, keys: &[String], found: imagery::Scan) -> ImageryResponse {
    let empty = found.rasters.is_empty();
    // Spare detailed rasters follow the chosen one with the same role: the
    // consumer tries them in order when the chosen one did not describe
    // itself (see `imagery::spares`).
    let rasters = found
        .rasters
        .into_iter()
        .chain(found.alternates.into_iter().map(|key| (key, imagery::Role::Detailed)))
        .map(|(key, role)| raster(key, role, keys))
        .collect::<Vec<_>>();

    // The frame only when it is visible in the name (Sentinel-2 tile). A
    // parse error does not fail the answer: without a frame the consumer
    // lives on the catalogue footprint.
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

/// A catalogue product by a storage key. Climbing to the root is knowledge
/// of the bucket layout (s3::product_root); after that it is an ordinary
/// catalogue request, only by exact name and for a single product.
/// Scene roots for a batch of keys — an answer that needs nowhere to go: the
/// scene boundary is set by the bucket layout and is visible in the key
/// itself.
///
/// Keys that lie in no scene do not enter the answer at all
/// (see `ProductRoots.roots`).
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
        // Answered, and there is nothing to repeat: an empty key stays empty.
        answer(
            &veldsdk::correlation(),
            None,
            "empty identifier: nothing to look up in the catalogue".to_string(),
            true,
        );
        return;
    }

    // No root — the key lies in a path to the scenes, not in a scene; we ask
    // the catalogue about the key itself: the catalogue knows the name of a
    // single object, while a year folder has no name, and the answer will be
    // an honest "not found".
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
    // The delimiter is the whole difference between a level and a subtree:
    // with it the storage answers with folders as common prefixes, without it
    // it unfolds all keys under the path.
    let listing = match request.recursive {
        true => s3::listing_deep(identity, &request.path, &request.token),
        false => s3::listing(identity, &request.path, &request.token),
    };
    log::info!(target: "handlers", "S3 listing: {}", listing.url);

    ask(state, veldsdk::correlation(), listing.url, listing.headers, Asked::List(request.path));
}

// ── Network answers ────────────────────────────────────────────

/// Parse the storage answer, or say why there is nothing to parse.
///
/// The storage reports refusal with a single status code — it has no body
/// with an explanation, and the code is all that can be shown here.
fn from_storage<T>(
    response: &veldsdk::proto::network::HttpTaskResponse,
    parse: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> Result<T, String> {
    match (200..300).contains(&response.status) {
        true => parse(&response.body).map_err(|error| error.to_string()),
        false => Err(format!("storage answered {}", response.status)),
    }
}

/// The same for the catalogue, with one difference: it explains refusals with
/// the response body, and the explanation is more useful than the code —
/// "bad filter" and "no such collection" look alike.
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

/// A network answer. Whose it is — a listing's or a search's — is recorded in
/// the wait itself, and it is parsed by whoever asked for it.
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
                            // A whole scene goes onto the globe; the listing
                            // knows no processing level — only the catalogue
                            // reports it.
                            let itself = product == entry.identifier.trim_end_matches('/');
                            let unviewable = match itself {
                                // The listing knows the product type no better
                                // than the level: it is not in the paths, and
                                // parsing the name for it is guessing where
                                // the catalogue answers precisely.
                                true => imagery::unviewable(
                                    &entry.identifier,
                                    entry.identifier.ends_with('/'),
                                    None,
                                    "",
                                ),
                                // This reason never reaches the viewer and
                                // cannot: an entry that turns out not to be a
                                // scene gets no icons at all. It is named
                                // because the field answers "why not", and
                                // emptiness in it would mean "allowed" — a
                                // lie that would be read one day.
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
                    // Not the last page — the same wait goes after the next
                    // one; it is too early to answer the requester.
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
                    // The traversal is over. The manifest is asked only
                    // where without it the choice would be a guess: for known
                    // layouts (Sentinel-1, Sentinel-2) the detailed raster is
                    // already named by their own rule, while the manifest is
                    // another signed request for hundreds of kilobytes, and
                    // for OLCI full resolution one of a megabyte and a half.
                    let known = imagery::scan(&identifier, &keys, &[], downloaded);
                    // Keys are certainly present here: without them the
                    // traversal itself would not have happened. This clause
                    // is never false, and it stands instead of `unwrap` not
                    // for reliability but by the price of a miss: `unwrap`
                    // would drop the module with a trap, and a trap carries
                    // away its whole state — for something that never
                    // happens. Should the chain fail to compose, the answer
                    // is assembled from file names, just as without a
                    // manifest at all (stated below).
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
            // The manifest was not fetched — this is not a refusal to the
            // product: the layout is parsed by file names, as before the
            // manifest. It is worth saying so: the raster choice is then
            // explained by a different rule.
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

            // The freshness window is over and the page did not fill up —
            // that means we set the bound, and we must lift it too. It is over
            // when the catalogue returned fewer products than asked: a full
            // page of products with a short page of scenes means not an empty
            // window but a folding — there is more, one just has to keep
            // paging. The window is not set a second time: the slow catalogue
            // answer is paid for with work here (see `catalogue::FRESH_DAYS`).
            let want = catalogue::wanted(&request) as usize;
            let drained = raw < catalogue::asked(&request) as usize;
            // The upper bound counts on par with the lower: we set no bound of
            // our own where the requester named only "until" (see
            // `catalogue::search`) — while a repeat of the same request would
            // go to the network byte for byte the same and return the same.
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
                // And this is not an answer at all — the query could not be
                // made.
                Err(error) => {
                    log::warn!(target: "handlers", "Product '{}' not found: {}", name, error);
                    answer(&pending.correlation_id, None, error, false);
                    return;
                }
            };

            let Some((facts, product)) = found.into_iter().next() else {
                // An empty answer is also an answer: the key is not from the
                // catalogue (climate data, auxiliary data) or the product has
                // already left it.
                answer(
                    &pending.correlation_id,
                    None,
                    format!("no product named '{}' in the catalogue", name),
                    true,
                );
                return;
            };

            // One part was asked for, but the scene should be shown — that is,
            // the part fit for it. The catalogue knows the neighbors, and this
            // is one more move to it, not an answer.
            // Neighbors are worth asking only where the catalogue tied the
            // product to anything at all: if it did not, the product has no
            // neighbors either, and the move to the catalogue is wasted.
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
            // Neighboring parts are an addition, not an answer: if they are
            // not found or not asked for, the requester gets what was found
            // as is. Refusing because of the neighbors would lose the product
            // itself.
            //
            // They are picked by the same rule the search folds scenes with
            // (`scene::same_scene`), not by the bare key: parts without a
            // slice number have diverging keys, and the same row would unfold
            // differently depending on where it was asked about.
            let others = from_catalogue(&response, catalogue::parse).unwrap_or_default();
            // Hitting the output ceiling is not a refusal but a quiet loss: we
            // set no order, and what gets cut is random, including the very
            // part the move was made for. There is no one else to tell: the
            // requester will receive a scene, simply not the right one.
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
