//! Overlays on the globe: tie "which product -> which files -> show" together.
//!
//! Exactly the same boundary as for outlines: the provider knows the product
//! layout and gives out raster roles with the frame, the globe can drape
//! opened resources over the binding — and only the one who holds both the
//! list and the globe can tie one to the other. The provider opens the
//! resources (the signature is only theirs), ownership goes to the globe
//! together with the overlay set.
//!
//! There are many overlays, and they travel to the globe as a whole set — not
//! because it is cheaper, but because that is its contract: what was not sent
//! is gone. Hence the only rule of this module: **every change of the set ends
//! with sending the set** ([`send_set`]). There is no way to append one overlay
//! or fade one, and inventing a second way for that would mean keeping the set
//! in two places at once.
//!
//! Asking "did the set change" before sending is not needed: the topic is
//! declared as a snapshot, and one matching the previous does not reach the
//! bus. The rule therefore holds without caveats — always send.
//!
//! Already sent rasters are resent with the same descriptors: there is no
//! ownership over them, these are names by which the globe recognizes the
//! previous overlay and does not reopen it.

use crate::module::footprint;
use crate::module::state::{
    overlay::Assembly, overlay::OverlayState, overlay::Part, overlay::Progress, Locate, Located,
    Open, Shift,
    State, ViewId,
};
use crate::proto::data_provider::{
    DataProduct, ImageryRequest, ImageryResponse, LocateRequest, LocateResponse,
};
use crate::proto::globe::{
    GeoPoint, Overlay, OverlayRaster, OverlayRole, Overlays, UtmFrame,
};
use veldsdk::proto::core::ResourceOpened;

/// From which longitude span a footprint stops being a quadrilateral and
/// becomes a belt around the Earth. An acquisition swath is never this wide:
/// the widest granule is four hundred kilometers, and an orbit crosses
/// meridians while staying a strip, fitting the longitude extent with a large
/// margin.
const WHOLE_EARTH_DEG: f64 = 350.0;

/// The raster role at the provider and the raster role at the globe are two
/// different enums in two different contracts, and they are linked here, not
/// by matching numbers.
///
/// A match is not a link: it holds exactly until the day an enum gets a new
/// variant in the middle, and then the detailed raster will travel as the
/// preview, and the compiler will not say a word. The `match` will.
fn role_for_globe(role: crate::proto::data_provider::ImageryRole) -> OverlayRole {
    use crate::proto::data_provider::ImageryRole;
    match role {
        ImageryRole::ImageryPreview => OverlayRole::OverlayPreview,
        ImageryRole::ImageryDetailed => OverlayRole::OverlayDetailed,
    }
}

/// The last segment of the storage key is the file name shown in the layer
/// row. A folder key ending in a slash is named by the folder.
fn file_name(identifier: &str) -> &str {
    identifier.trim_end_matches('/').rsplit('/').next().unwrap_or_default()
}

/// "On the globe" from a list row — and back: the same icon removes what it
/// placed.
///
/// A toggle, not a one-way show, and with the exact mechanics of the outline
/// icon: the snapshot is either on the globe or it is not — one question, and
/// answering it with a press here and a button in another list back would mean
/// giving one state two levers in different places.
///
/// It does not move the camera. Aiming is a separate intention and a separate
/// button (`Msg::OutlineFocus`): "show here" and "take me there" are different
/// requests, and merged into one they take the answer from each other — placing
/// a second snapshot next to the first becomes impossible without flying off
/// to it.
///
/// The key arrives already without the listing's trailing slash: the row itself
/// normalizes it to the product form (see `Row::snapshot_key`), and a second
/// such rule would disagree with the first.
///
/// Showing does not tick the selection checkbox: these are different
/// intentions. Selection is for doing something with what is picked — outline,
/// delete, download — while showing is for looking, and what is shown lives in
/// its own list ("On view"). Ticking the checkbox along the way would put the
/// snapshot into the next batch action nobody asked for.
pub fn on_toggle_pressed(state: &mut State, view: ViewId, identifier: String) {
    // We close the row menu ourselves: what was pressed in it is done, and left
    // open it would keep hanging over the list.
    state.close_menus();
    // A previous refusal is cleared by a new attempt: keeping it further would
    // mean explaining with a cause what no longer happens — and the show could
    // fail over the network too, and then a second try succeeds.
    state.unshowable.remove(&identifier);
    // Already on the globe or on the way there — so they ask to remove it. A
    // hidden one counts the same as a visible one: it remains a layer, and the
    // icon in its row lights up just the same.
    if state.overlays.iter().any(|overlay| overlay.identifier == identifier) {
        remove(state, &identifier);
        return;
    }
    // The trip to the catalogue for the product cannot be killed — it will
    // answer anyway — but the show can become unwanted: the answer will ask
    // whether it is still awaited (see [`on_located`]).
    if state.showing.remove(&identifier) {
        veldsdk::log::info!(target: "handlers", "show of '{}' cancelled before the catalogue answered", identifier);
        // And the file wish with it: nobody waits for it.
        state.file_wishes.remove(&identifier);
        send_set(state);
        return;
    }
    // First the search results: there the product already has everything
    // needed, and there is no reason to go to the catalogue a second time.
    if super::search::show(state, view, &identifier) {
        super::nav::on_new_globe(state);
        return;
    }
    // The product has to be restored from the provider — and one trip answers
    // both: the overlay and the outline. The answer is marked as expected right
    // here, otherwise the outline assembly would send a second request for the
    // same product. What is already known is not overwritten: an outlined one
    // already has its geometry stored, and erasing it would mean an outline
    // missing for a frame.
    if !matches!(state.located.get(&identifier), Some(Located::Found(_))) {
        state.located.insert(identifier.clone(), Located::Asking);
    }
    state.showing.insert(identifier.clone());
    let correlation = state.locates.begin(Locate::Overlay(identifier.clone()));
    crate::calls::data_provider::on_locate(&LocateRequest { identifier }, &correlation);
    // The wish is already visible — by the icon in the row and the bar under
    // it (see `rows::onto_globe`) — and the snapshot's place on the globe will
    // show hatching if the geometry is at hand.
    send_set(state);
    super::nav::on_new_globe(state);
}

/// The product is restored by key — show it as we would from search.
///
/// The product key may turn out different from the one it was called with: a
/// file inside the snapshot was asked for, but the catalogue answers with the
/// product root (see `on_locate` at the provider). The overlay is placed under
/// the product key — otherwise the globe would get two keys for one snapshot.
pub fn on_located(state: &mut State, key: &str, response: LocateResponse) {
    // While the trip to the catalogue was in flight, the show could become
    // unwanted — "remove from the globe" gets pressed within those seconds.
    // The request cannot be killed in flight, it will answer anyway, so we
    // ask here whether it is still awaited. Otherwise, seconds after the
    // cancellation, the application itself would place a snapshot on the globe
    // that nobody asked for.
    //
    // A closed search tab does not cancel the show: the layer lives in the
    // module state and its own list, not the one it was called from.
    if !state.showing.remove(key) {
        veldsdk::log::info!(target: "handlers", "show of '{}' became unwanted while the answer was in flight", key);
        return;
    }
    let Some(product) = response.product else {
        veldsdk::log::warn!(target: "handlers", "could not show on the globe: {}", response.error);
        state.notice = Some(format!("Could not show on the globe: {}", response.error));
        // The wish is gone — so there is nothing left to hatch (see
        // [`send_set`]); and the file it asked to lie with has nothing to wait
        // for.
        state.file_wishes.remove(key);
        send_set(state);
        return;
    };

    // The answer arrived under a different key, and what was asked under this
    // one no longer means anything: the next one who needs the file geometry
    // will ask again. Except when this key is outlined: there the answer is
    // awaited, and forgotten it would go to the catalogue a second time for
    // the same thing.
    if key != product.identifier && !state.outlines.contains(key) {
        state.located.remove(key);
    }
    // The product is cached under its own key — the one it is now addressed
    // by: without this the outline assembly would consider it unasked and send
    // a second request to the catalogue for the same thing.
    state.located.insert(product.identifier.clone(), Located::Found(product.clone()));
    show(state, &product, None);
}

/// Place a product on the globe. An already placed one is not placed again: a
/// rebuild would cost reopening the rasters, and it is on the globe anyway.
///
/// The new layer goes on top of the previous ones — at the end of the set,
/// because that is what was asked to be shown.
///
/// `source` — the search view whose results the product was taken from; for a
/// product restored by key there is none (see `OverlayState::source`).
pub fn show(state: &mut State, product: &DataProduct, source: Option<ViewId>) {
    // A previous refusal is cleared by every new attempt to show, whichever way
    // it comes — icon, outline, or search results. It must be cleared here, in
    // the common funnel: there are three entrances to showing, and a reason left
    // after a successful show would outlive it and surface as ochre on a
    // removed layer.
    state.unshowable.remove(&product.identifier);
    if state.overlays.iter().any(|overlay| overlay.identifier == product.identifier) {
        // Nothing to place, but something to say: this is also reached with a
        // catalogue answer that cancelled the wish (see [`on_located`]) — and
        // the wish is marked on the globe with hatching, and nobody else can
        // clear it.
        send_set(state);
        return;
    }

    let mut overlay = OverlayState::new(
        product.identifier.clone(),
        product.name.clone(),
        product.folder,
        source,
        quad_of(product),
        footprint::frame(&product.footprint),
    );
    // The wish to place the snapshot with this file waited for the product
    // here (see `State::file_wishes`); the layer carries it on.
    overlay.wanted_file = state.file_wishes.remove(&product.identifier);
    state.overlays.push(overlay);

    request_imagery(state, &product.identifier);
    // The overlay set itself has not changed — the one being assembled does
    // not enter it — but what is visible on the globe has: the snapshot's
    // place is marked with hatching while there is no picture yet (see
    // [`send_set`]).
    send_set(state);
}

/// Ask the provider for the layer's rasters — with the file we were asked to
/// place, if any. One place for the first request and for a repeat with a
/// changed file.
fn request_imagery(state: &mut State, key: &str) {
    let correlation = state.imageries.begin(key.to_string());
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.imagery = Some(correlation.clone());
    let request = imagery_request(&state.library, overlay);
    crate::calls::data_provider::on_imagery(&request, &correlation);
}

/// What to ask the layer's rasters with: the snapshot key, whether it is
/// downloaded, and the file we were asked to place.
fn imagery_request(library: &crate::module::state::LibraryState, overlay: &OverlayState) -> ImageryRequest {
    ImageryRequest {
        // Whether the snapshot is downloaded is known only to us: the provider
        // has a single store. The raster set depends on it — the detailed one,
        // readable in a single pass, is only offered when the file is at hand.
        downloaded: library.whole(&overlay.identifier),
        identifier: overlay.identifier.clone(),
        wanted: overlay.wanted_file.clone().unwrap_or_default(),
    }
}

/// Place the snapshot on the globe with this file — the file menu item inside
/// a snapshot.
///
/// For an already placed layer the rasters are re-asked with the named file:
/// other resources will arrive, and the globe will replace the set (see its
/// `adopt_overlay`); the previous one stays until the replacement. If there is
/// no layer yet, the product is restored by the same trip as via the globe
/// icon, and the wish waits for it in `State::file_wishes`. If an assembly or
/// a raster request is in flight, the wish waits for their end: a second
/// assembly on top of the first would leave its opens without an owner. A
/// cancelled show, a catalogue refusal, and a removed layer take the wish with
/// them.
pub fn on_file_pressed(state: &mut State, product: &str, file: String) {
    state.close_menus();
    place_file(state, product, file);
}

/// Place the snapshot with this file — without closing the menu: this is also
/// where a wish that waited out the assembly comes (see [`finish`]).
fn place_file(state: &mut State, product: &str, file: String) {
    state.unshowable.remove(product);
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == product) {
        if overlay.wanted_file.as_deref() == Some(file.as_str()) {
            return;
        }
        if overlay.imagery.is_some() || overlay.assembly.is_some() {
            veldsdk::log::info!(target: "handlers", "'{}': rasters are still being assembled — file {} will wait", overlay.label, file_name(&file));
            state.file_wishes.insert(product.to_string(), file);
            return;
        }
        overlay.wanted_file = Some(file);
        overlay.variable_wanted = None;
        request_imagery(state, product);
        return;
    }
    state.file_wishes.insert(product.to_string(), file);
    if state.showing.contains(product) {
        return;
    }
    let found = match state.located.get(product) {
        Some(Located::Found(found)) => Some(found.clone()),
        _ => None,
    };
    if let Some(found) = found {
        show(state, &found, None);
        super::nav::on_new_globe(state);
        return;
    }
    state.located.insert(product.to_string(), Located::Asking);
    state.showing.insert(product.to_string());
    let correlation = state.locates.begin(Locate::Overlay(product.to_string()));
    crate::calls::data_provider::on_locate(&LocateRequest { identifier: product.to_string() }, &correlation);
    send_set(state);
    super::nav::on_new_globe(state);
}

/// Remove one overlay: release the resources, resend the set.
pub fn remove(state: &mut State, key: &str) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let overlay = state.overlays.remove(index);
    abandon(state, overlay);
    // The removed layer could be the one circled by the ribbon: without it the
    // ribbon has nothing to hold on to (see `outline::forget_gone`).
    super::outline::forget_gone(state);
    send_set(state);
}

/// Move a layer one place within the set. The set order is also the layer
/// order on the globe from bottom to top, so the whole rearrangement ends
/// here: the globe receives the set whole and lays it out in the order sent.
///
/// The direction here is about the set, not the screen: the "On view" list is
/// reversed (newest on top), and the reversal lives there (see `view::shown`).
/// The end layer does not move, and silently — bumping into the end is not an
/// error.
pub fn shift(state: &mut State, key: &str, shift: Shift) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let target = match shift {
        Shift::Up if index + 1 < state.overlays.len() => index + 1,
        Shift::Down if index > 0 => index - 1,
        _ => return,
    };
    state.overlays.swap(index, target);
    send_set(state);
}

/// Remove everything from the globe.
pub fn clear_all(state: &mut State) {
    // And what is still in flight: the catalogue answer will arrive anyway, and
    // placing a snapshot after "remove all" would mean returning to the globe
    // what was just removed from it.
    state.showing.clear();
    for overlay in std::mem::take(&mut state.overlays) {
        abandon(state, overlay);
    }
    super::outline::forget_gone(state);
    send_set(state);
}

/// The end of one overlay. The globe frees the sent rasters — it owns them,
/// and the next set without this key will free them; ours remain only with an
/// assembly broken off midway.
fn abandon(state: &mut State, overlay: OverlayState) {
    // The raster request is taken off the ledger here, the same as the opens:
    // otherwise its answer would be recognized as a layer placed under the
    // same key again and would start a second assembly on top of the first.
    if let Some(correlation) = &overlay.imagery {
        state.imageries.take(correlation);
    }
    // The file wish that waited for the assembly to end leaves with the layer:
    // a removed layer will not fulfil it, and the next show of the same
    // snapshot did not ask for it.
    state.file_wishes.remove(&overlay.identifier);
    let Some(assembly) = overlay.assembly else { return };
    // In-flight opens are taken off the common ledger here: otherwise their
    // answer would be recognized by the next assembly of the same key, and its
    // resource would land in a foreign set. The removed one reaches the common
    // discard in module::on_open_result.
    for correlation in &assembly.opens {
        // The correlation stays on the ledger, but with an empty key: there is
        // no overlay with such a key, and the answer arriving later takes the
        // "overlay was removed" branch — the resource in it is ours, there it
        // is accepted and released. Taking it off the ledger entirely is not
        // allowed: an unrecognized answer goes to `discard`, and that one does
        // not touch the resource — the file would stay open until the end of
        // the session.
        if let Some((_, role, ordinal, part)) = state.opens.take(correlation) {
            state.opens.insert(correlation.clone(), (String::new(), role, ordinal, part));
        }
    }
    for (.., handle) in assembly.collected {
        veldsdk::resource::release(handle);
    }
}

/// An overlay survives a new search in its own view only if its product is
/// still in the results — the same rule as the marker (see
/// `search::survivors`). A foreign view's results do not touch overlays: a
/// product from the catalogue or downloads was never in it.
pub fn keep_only(state: &mut State, view: ViewId, alive: impl Fn(&str) -> bool) {
    retain(state, |overlay| {
        overlay.source != Some(view) || alive(&overlay.identifier)
    });
}

/// The search tab closed — overlays from its results leave with it. Those
/// restored by key live on: their source is not a tab.
pub fn source_closed(state: &mut State, view: ViewId) {
    retain(state, |overlay| overlay.source != Some(view));
}

/// Keep those that pass the condition and resend the set.
///
/// The removed one is spoken of aloud: it is removed by a rule ("an overlay
/// lives the life of its results"), not a person, and from outside a silent
/// disappearance of a snapshot from the globe looks like it unloaded itself.
fn retain(state: &mut State, keep: impl Fn(&OverlayState) -> bool) {
    let (kept, gone): (Vec<_>, Vec<_>) =
        std::mem::take(&mut state.overlays).into_iter().partition(&keep);
    state.overlays = kept;
    let said = match gone.as_slice() {
        [] => None,
        [one] => Some(format!(
            "\"{}\" removed from the globe: it is no longer in the results",
            crate::module::components::format::ellipsize(&one.label, 40)
        )),
        many => Some(format!(
            "{} {} removed from the globe: they are no longer in the results",
            many.len(),
            crate::module::components::format::plural(
                many.len(),
                ["snapshot", "snapshots", "snapshots"]
            )
        )),
    };
    for overlay in gone {
        abandon(state, overlay);
    }
    if let Some(said) = said {
        state.notice = Some(said);
    }
    super::outline::forget_gone(state);
    send_set(state);
}

// ── Showing ──────────────────────────────────────────────────────

/// Layer opacity. The set is resent whole, but the rasters in it are the same
/// descriptors, so moving the slider reopens nothing.
pub fn set_opacity(state: &mut State, key: &str, opacity: f32) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.opacity = opacity.clamp(0.0, 1.0);
    send_set(state);
}

/// Hide or show the layer.
pub fn set_hidden(state: &mut State, key: &str, hidden: bool) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.hidden = hidden;
    send_set(state);
}

/// Layer menu: expand or collapse the expanded one.
pub fn menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::Layer(key)),
        None => state.close_menus(),
    }
}

/// Hide or show all at once — the "Hide all" button in the list.
pub fn hide_all(state: &mut State, hidden: bool) {
    for overlay in &mut state.overlays {
        overlay.hidden = hidden;
    }
    send_set(state);
}

// ── Assembly ────────────────────────────────────────────────────

/// Giving up on an overlay means removing it, the same way as via the button:
/// a blank row that will never show anything is worse than its absence. Exactly
/// `remove`, not dropping it from the list: the layer may already lie on the
/// globe, and then it has to be taken off there too.
///
/// The reason also goes to the screen: "on the globe" was pressed, and nothing
/// appeared on the globe — and without a notice this looks like a button that
/// did not work. One function for all assembly refusals, because from outside
/// they are indistinguishable: the layer is gone, and the person has a right to
/// know why.
fn give_up(state: &mut State, key: &str, label: &str, why: String) {
    veldsdk::log::warn!(target: "handlers", "'{}': {}", label, why);
    // It must be said in two places, and this is not duplication: the questions
    // differ. The status line answers "what happened just now" — the one who
    // pressed the icon is looking at the globe at that moment, not the list, and
    // there is no other place to tell them. The key's memory answers "what
    // snapshot is this": the reason outlives the word in the status line and
    // meets the one who returned to the list — otherwise they see the same
    // inviting icon and await a refusal again.
    state.notice =
        Some(format!("\"{}\": {}", crate::module::components::format::ellipsize(label, 40), why));
    state.unshowable.insert(key.to_string(), why);
    remove(state, key);
}

/// The provider's answer: raster roles and the frame. We open every raster as
/// a resource.
pub fn on_imagery_result(state: &mut State, response: ImageryResponse) {
    let Some(key) = state.imageries.take(&veldsdk::correlation()) else { return };
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { return };
    let label = overlay.label.clone();
    // An assembly is already running — so this is the answer to a request that
    // was not fully taken off the ledger. A second one must not be started on
    // top of the first: its opens would be left without an owner, and their
    // resources would land in a foreign set.
    if overlay.assembly.is_some() {
        veldsdk::log::warn!(target: "handlers", "'{}': rasters are already being assembled", label);
        return;
    }

    if !response.error.is_empty() {
        return give_up(state, &key, &label, format!("rasters could not be requested: {}", response.error));
    }
    // The named file was refused — with words on the screen and the wish
    // cleared: otherwise pressing the same item again would be empty, and the
    // layout file that did land would be unexplained.
    if !response.wanted_refused.is_empty() {
        state.notice = Some(format!("«{}»: {}", crate::module::components::format::ellipsize(&label, 40), response.wanted_refused));
        if let Some(named) = state.overlays.iter_mut().find(|o| o.identifier == key) {
            named.wanted_file = None;
        }
    }
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { return };
    if response.rasters.is_empty() {
        // The reason is named by the provider — only it has seen what lies in
        // the product. Our own phrase stays for when it spoke none: an empty
        // string on the screen is worse than a generic one.
        let why = match response.reason.is_empty() {
            true => "no rasters to overlay".to_string(),
            false => response.reason,
        };
        return give_up(state, &key, &label, why);
    }
    // There is no geometry at all — neither frame nor footprint: the snapshot
    // has nowhere to lie even approximately, and there is no reason to open the
    // rasters. A complex footprint does not apply here: the raster itself will
    // bring the binding, and before it the binding is unknown (see
    // [`quad_of`]).
    if response.utm.is_none() && overlay.quad.is_none() {
        return give_up(state, &key, &label, "no binding: the product has no geometry".to_string());
    }

    let utm = response.utm.map(|utm| UtmFrame {
        zone: utm.zone,
        south: utm.south,
        x0: utm.x0,
        y0: utm.y0,
        x1: utm.x1,
        y1: utm.y1,
    });
    let mut opens = Vec::new();
    // File names by their number in the answer — by them the one lying as
    // detailed is named later.
    let files: Vec<String> = response.rasters.iter().map(|raster| raster.identifier.clone()).collect();
    // The raster's ordinal in the answer is how the coordinates find their
    // raster: there can be several detailed ones (the chosen one and spares),
    // and the role is not enough for that.
    for (ordinal, raster) in (0u32..).zip(response.rasters) {
        let role = role_for_globe(raster.role());
        // Coordinates are opened in the same order and on equal terms with the
        // raster: for Sentinel-3 they lie in a neighboring file, and without
        // them the swath has nowhere to lie. Empty means the coordinates are in
        // the raster itself or absent altogether.
        let files = [(Part::Raster, raster.identifier)]
            .into_iter()
            .chain((!raster.geolocation.is_empty()).then_some((
                Part::Geolocation,
                raster.geolocation,
            )));
        for (part, identifier) in files {
            // What to open with is decided by the file, not the snapshot: a
            // product may be partly downloaded, and a quicklook lying nearby
            // does not make the measurement raster local. The rule is shared
            // with the row preview — it is in
            // `LibraryState::local_name`.
            let local = state.library.local_name(&identifier).map(str::to_string);
            // The role is translated right away, at the boundary: further on,
            // our code carries the one the globe understands.
            let correlation = state.opens.begin((key.clone(), role, ordinal, part));
            opens.push(correlation.clone());
            super::open_resource(identifier, local, &correlation);
        }
    }

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    // The raster request is over — its correlation is no longer ours.
    overlay.imagery = None;
    overlay.assembly = Some(Assembly { utm, files, opens, collected: Vec::new() });
}

/// Opening of an overlay raster. `false` — the answer is not ours: a foreign
/// correlation or an assembly that was already broken off (its opens were
/// taken off the ledger in `abandon`), and the arriving resource reaches the
/// common discard (see module::on_open_result).
pub fn on_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation = veldsdk::correlation();
    let Some((key, role, ordinal, part)) = state.opens.take(&correlation) else { return false };

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else {
        // The overlay was removed between the request and the answer: the
        // resource is ours, and there is nobody else to release it.
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };
    // There is no assembly — it ended (or was broken off) before this answer
    // arrived. The resource is ours anyway: nobody else will accept and release
    // it.
    let Some(assembly) = overlay.assembly.as_mut() else {
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };

    assembly.opens.retain(|waiting| waiting != &correlation);
    match veldsdk::resource::accept(opened) {
        Ok(handle) => assembly.collected.push((role, ordinal, part, handle)),
        // It did not open — the overlay lives on what did open: without the
        // raster there is no role, without the coordinates there is no
        // binding, and both outcomes are explained below.
        Err(error) => veldsdk::log::warn!(target: "handlers", "overlay file: {}", error),
    }
    if assembly.opens.is_empty() {
        finish(state, &key);
    }
    true
}

/// The collected items into "raster and its coordinates" pairs, previews
/// first. The second answer is the coordinates with no raster found for them:
/// for the overlay they are not a file but a raster property, and the caller
/// releases them while they are still theirs.
///
/// The order here is not decoration but the price of showing: the globe sends
/// descriptions out one after another through the set
/// (`globe::module::adopt_overlay`), while the tiler describes one at a time,
/// so the first in the list holds up the rest. A preview is a quicklook of
/// hundreds of kilobytes, a detailed one a granule of megabytes, and over the
/// network there are tens of seconds between them. The provider puts the
/// preview first, and the order is lost in between: the opens go out at once,
/// the network answers arrive out of order, and the collected items lie in the
/// order of their arrival.
///
/// This brings the first picture closer for not every layer: with a quad
/// binding the globe draws nothing until both rasters are described
/// (`overlay::binding_pending`) — there the order only decides what frees the
/// tiler queue sooner.
///
/// There can be several detailed rasters — the chosen one and the spares after
/// it, in the provider's order (see `imagery::spares`) — and the coordinates
/// find their raster by the ordinal in the answer, not by the role: the role
/// is the same for the spares. The order of ordinals within a role is
/// preserved — it is the order of preference.
fn paired<H>(
    collected: Vec<(OverlayRole, u32, Part, H)>,
) -> (Vec<(OverlayRole, u32, H, Option<H>)>, Vec<H>) {
    let mut pairs: Vec<(OverlayRole, u32, H, Option<H>)> = Vec::new();
    let mut coordinates = Vec::new();
    for (role, ordinal, part, handle) in collected {
        match part {
            Part::Raster => pairs.push((role, ordinal, handle, None)),
            Part::Geolocation => coordinates.push((ordinal, handle)),
        }
    }
    pairs.sort_by_key(|(role, ordinal, ..)| (*role as i32, *ordinal));

    let mut orphans = Vec::new();
    for (ordinal, handle) in coordinates {
        match pairs.iter_mut().find(|(_, other, ..)| *other == ordinal) {
            Some(pair) => pair.3 = Some(handle),
            None => orphans.push(handle),
        }
    }
    (pairs, orphans)
}

/// All opens of this overlay are done — hand ownership to the globe and
/// resend the set.
fn finish(state: &mut State, key: &str) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    let Some(assembly) = overlay.assembly.take() else { return };
    let label = overlay.label.clone();

    // The "raster and its coordinates" pairs are assembled BEFORE ownership is
    // handed over. The order here is not a matter of taste: extra items can be
    // released only while they are ours, and after `hand_off` the resource
    // belongs to the globe — the host will reject the release, and nobody will
    // remember the abandoned file.
    let (pairs, orphans) = paired(assembly.collected);
    // Coordinates by themselves are not a file for the overlay but a raster
    // property: the raster did not open — there is nothing to show by them.
    for handle in orphans {
        veldsdk::log::warn!(target: "handlers", "'{}': coordinates without a raster — releasing", label);
        veldsdk::resource::release(handle);
    }

    // Ownership hand-off comes before the message: having received the set,
    // the globe may immediately consider the resources its own. On refusal the
    // helper has freed the resource itself.
    let mut rasters = Vec::new();
    for (role, ordinal, raster, coordinates) in pairs {
        let raster = match veldsdk::resource::hand_off(raster, "globe") {
            Ok(handle) => handle,
            Err(error) => {
                veldsdk::log::warn!(target: "handlers", "the raster was not handed off to the globe: {}", error);
                // This raster's coordinates are needed by nobody else, and
                // they are still ours — release them while there is something
                // to release.
                if let Some(handle) = coordinates {
                    veldsdk::resource::release(handle);
                }
                continue;
            }
        };
        let coordinates = coordinates.and_then(|handle| {
            match veldsdk::resource::hand_off(handle, "globe") {
                Ok(handle) => Some(handle),
                // The raster is already with the globe and will lie by the
                // catalogue footprint: it will have no binding from the file,
                // but the layer will come to be.
                Err(error) => {
                    veldsdk::log::warn!(target: "handlers",
                        "the coordinates were not handed off to the globe: {}", error);
                    None
                }
            }
        });
        rasters.push(OverlayRaster {
            resource: Some(raster),
            role: role as i32,
            geolocation: coordinates,
            ordinal,
            variable: String::new(),
        });
    }
    if rasters.is_empty() {
        // By the same path and with the same word as the other assembly
        // refusals (see `give_up`): a blank row that will never show anything
        // is worse than its absence, and the icon was pressed — yet nothing
        // appeared on the globe.
        return give_up(state, &key, &label, "not a single raster opened".to_string());
    }

    overlay.utm = assembly.utm;
    overlay.rasters = rasters;
    overlay.files = assembly.files;
    // The work has not ended but begun: the globe will receive the rasters and
    // start describing them — over the network that is tens of seconds. We
    // have no word of our own about the progress and never will, it will be
    // said by the globe itself (`on_overlay_progress`), but a frame passes
    // before its first word, and the calm left for that frame would dim both
    // the bar under the row and the hatch on the globe — visibly and for no
    // reason.
    overlay.progress = Progress { working: true, blank: true, ..Progress::default() };
    send_set(state);
    // A file wish that arrived during the assembly is fulfilled here: the set
    // that just left for the globe will be replaced by the next one — one
    // description wasted, the price of not losing the wish.
    if let Some(file) = state.file_wishes.remove(key) {
        place_file(state, key, file);
    }
}

/// Send the whole set to the globe. The only way to tell it anything about
/// overlays — hence one call site for every change.
///
/// Ones being assembled do not enter the set: the globe cannot accept an
/// overlay without rasters, and sending it would mean asking to remove what it
/// has not seen yet. But they are exactly that half of the set that is visible
/// on the globe as an outline: the place of the incoming snapshot is marked
/// with a hatch while there is no picture yet — so the outline set leaves from
/// here too (see `outline::send`). Two sets, but one change: an overlay being
/// assembled changes exactly what is visible on the globe.
fn send_set(state: &State) {
    let overlays = state
        .overlays
        .iter()
        .filter(|overlay| overlay.on_globe())
        .map(|overlay| {
            // The quad is the fallback binding; with a frame the globe does
            // not need it. The roughness flag is decided by the same move and
            // no other: told with an empty quad, it would speak of a
            // quadrilateral that is not in the message.
            let (quad, rough) = match (overlay.utm.is_some(), overlay.quad) {
                (false, Some(points)) => (
                    points.map(|(lat, lon)| GeoPoint { lat, lon }).to_vec(),
                    overlay.rough,
                ),
                _ => (Vec::new(), false),
            };
            Overlay {
                key: overlay.identifier.clone(),
                label: overlay.label.clone(),
                utm: overlay.utm.clone(),
                quad,
                rough,
                rasters: with_variable(&overlay.rasters, overlay.variable_wanted.as_ref()),
                opacity: Some(overlay.opacity),
                hidden: overlay.hidden,
            }
        })
        .collect();

    crate::calls::globe::on_overlay(&Overlays { overlays });
    super::outline::send(state);
}

/// The set's rasters carry the named variable for the one it is told of — by
/// file number; empty for the rest (the tiler's choice).
fn with_variable(rasters: &[OverlayRaster], wanted: Option<&(u32, String)>) -> Vec<OverlayRaster> {
    rasters
        .iter()
        .map(|raster| OverlayRaster {
            variable: match wanted {
                Some((ordinal, path)) if *ordinal == raster.ordinal => path.clone(),
                _ => String::new(),
            },
            ..raster.clone()
        })
        .collect()
}

/// The variable list in the layer row: expand, close by a click away or a
/// second press. Only one is expanded on screen (see `state::Open`).
pub fn variables_menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::LayerVariables(key)),
        None => state.close_menus(),
    }
}

/// Another variable of the layer's file lying as detailed — to the globe with
/// the same set: it will re-describe the raster with the named variable, the
/// resources stay the same. If it is already the shown one — nothing to do but
/// close the list.
pub fn variable(state: &mut State, key: &str, variable: String) {
    state.close_menus();
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    if overlay.variable.as_ref().is_some_and(|shown| shown.path == variable) {
        return;
    }
    let Some(ordinal) = overlay.detailed_ordinal else { return };
    if overlay.variable_wanted.as_ref() == Some(&(ordinal, variable.clone())) {
        return;
    }
    overlay.variable_wanted = Some((ordinal, variable));
    send_set(state);
}

/// Tile extraction progress from the globe: the whole set, with the same rule
/// as the overlay set itself — whoever is not mentioned is not extracted for.
///
/// The numbers arrive ready-made, and the layer list and the snapshot row show
/// them in every list where it appears (`rows::onto_globe`). But one of them
/// changes the globe itself: while there is nothing to draw for a layer, its
/// place is hatched, and only the "there is something now" coming from here
/// can clear the hatch (see `Progress::blank`).
/// Hence the outline set is resent right here.
pub fn on_overlay_progress(state: &mut State, msg: crate::proto::globe::OverlaysProgress) {
    for overlay in &mut state.overlays {
        // What is not mentioned, we do not touch. Silence here means not "the
        // work is done" but "the globe does not know about this layer yet": it
        // reports on all it has — and the layer just handed to it
        // (`finish`) is not in its set for the first frames. Erased by
        // silence, it would dim both the bar and the hatch for exactly those
        // frames.
        let Some(said) = msg.overlays.iter().find(|progress| progress.key == overlay.identifier)
        else {
            continue;
        };
        overlay.trouble = said.trouble.clone();
        overlay.detailed_trouble = said.detailed_trouble.clone();
        overlay.variable = said.detailed_variable.clone();
        overlay.detailed_ordinal = said.detailed;
        // The list comes from the last report that had it: a description that
        // refused the named variable carries no list, and choosing another one
        // is needed exactly then.
        if !said.detailed_variables.is_empty() {
            overlay.variables = said.detailed_variables.clone();
        }
        // A different file (a spare) landed as detailed — what was named
        // about the previous one does not apply to it.
        if overlay.variable_wanted.as_ref().is_some_and(|(ordinal, _)| said.detailed != Some(*ordinal)) {
            overlay.variable_wanted = None;
        }
        // The number from the globe becomes a file name: the globe knows no
        // names, and the layer row needs one, and it must stand next to the
        // verdict about the same file.
        overlay.detailed = said
            .detailed
            .and_then(|at| overlay.files.get(at as usize))
            .map(|identifier| file_name(identifier).to_string());
        overlay.progress = Progress {
            ready: said.ready,
            total: said.total,
            share: said.share,
            working: said.working,
            step: said.step,
            steps: said.steps,
            blank: said.blank,
            pass: (said.pass_read, said.pass_total),
        };
    }

    // A layer that has nothing to lie with is removed by the same path and
    // with the same word as the assembly refusals: the overlay set is ours,
    // and only the one who put a non-lying layer there can take it off. Only
    // the globe can notice this — it holds the rasters and the binding — and
    // "a blank row that will never show anything is worse than its absence" is
    // equally true from both sides.
    let doomed: Vec<(String, String)> = msg
        .overlays
        .iter()
        .filter(|progress| !progress.error.is_empty())
        .map(|progress| (progress.key.clone(), progress.error.clone()))
        .collect();
    for (key, why) in doomed {
        let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { continue };
        let label = overlay.label.clone();
        give_up(state, &key, &label, why);
    }

    // The overlay set does not change from the pass — what changes is what is
    // visible on the globe besides them. An unchanged set does not reach the
    // bus, so there is no price for "just in case" here.
    super::outline::send(state);
}

/// The fallback binding from the catalogue footprint and how far it can be
/// trusted.
///
/// Exactly four vertices are the corners of the captured piece, and the raster
/// is stretched over them (`rough: false`). Anything more complex is an
/// acquisition swath outlined by dozens of vertices: it has no corners, and
/// instead the extent of the inscribed circle is taken (`rough: true`).
/// Stretching over it is not allowed, and the globe does not do it — it holds
/// the place until the raster itself brings the binding (see `Frame::Rough`).
///
/// Refusing a complex footprint here would be premature: the binding lives in
/// the raster more often than in the catalogue — a Sentinel-5P granule carries
/// latitude and longitude per sample — and the raster can only be asked by
/// opening it.
///
/// A quadrilateral over the whole Earth cannot be described by corners either,
/// and this is not rare: a global climate grid is outlined by a rectangle from
/// −180 to 180, and on the globe that is one and the same meridian. There is
/// nothing to stretch over it — all four corners converge on one meridian — so
/// such a footprint goes the same way as an acquisition swath: as a place, not
/// as a binding.
fn quad_of(product: &DataProduct) -> Option<([(f64, f64); 4], bool)> {
    // There are several rings — the snapshot is cut by a seam: across the
    // antimeridian the catalogue returns it as two polygons, one half on each
    // side. Taking the first and stretching the whole raster over it would show
    // the snapshot twice as narrow as captured and on its own half; stitching
    // them back by the vertices lying exactly at ±180 is not possible — the
    // halves have their own traversal order. So this is not a quadrilateral,
    // and its path is the same as the acquisition swath's: a place, not a
    // binding.
    if product.footprint.len() == 1
        && let Some(ring) = product.footprint.first()
    {
        let points = match ring.points.as_slice() {
            [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
                &ring.points[..ring.points.len() - 1]
            }
            all => all,
        };
        if let [a, b, c, d] = points {
            let lons = [a.lon, b.lon, c.lon, d.lon];
            let span = lons.iter().fold(f64::MIN, |wide, lon| wide.max(*lon))
                - lons.iter().fold(f64::MAX, |narrow, lon| narrow.min(*lon));
            if span < WHOLE_EARTH_DEG {
                return Some((
                    oriented([(a.lat, a.lon), (b.lat, b.lon), (c.lat, c.lon), (d.lat, d.lon)]),
                    false,
                ));
            }
        }
    }
    // The circle the footprint is inscribed in is the only measure of a
    // complex footprint that does not lie at a pole or at the seam (see
    // `footprint::frame`).
    let circle = footprint::frame(&product.footprint)?;
    let (north, south) = (
        (circle.lat + circle.radius_deg).min(90.0),
        (circle.lat - circle.radius_deg).max(-90.0),
    );
    let (west, east) = (circle.lon - circle.radius_deg, circle.lon + circle.radius_deg);
    Some(([(north, west), (north, east), (south, east), (south, west)], true))
}

/// The quad vertices in raster traversal order — upper left, upper right,
/// lower right, lower left — as the globe reads them (`Overlay.quad`). The
/// catalogue lists the footprint corners in its own order, from any vertex and
/// in any direction, while the globe puts the first vertex into the raster's
/// upper left corner: the same snapshot would land mirrored or rotated,
/// depending on which corner the catalogue started from. The raster is assumed
/// to lie north up — that is how Sentinel-2 tiles lie (those, however, come
/// with a UTM frame and never reach the quad) and quicklooks; the Sentinel-1
/// quicklook lies in the acquisition geometry, and north up for it is an
/// assumption not checked by any scenario. For a swath skewed more than
/// forty-five degrees, two vertices share one quarter, and the vertices stay
/// as listed: the place of such a granule is refined by its own coordinate
/// grid anyway.
fn oriented(corners: [(f64, f64); 4]) -> [(f64, f64); 4] {
    // Longitudes are unwound toward the first vertex, as in the globe's
    // `Frame::quad`: for a footprint across the antimeridian the mean longitude
    // would otherwise land on the other side of the Earth, and west and east
    // would swap places.
    let base = corners[0].1;
    let unwound = |lon: f64| base + (lon - base + 540.0).rem_euclid(360.0) - 180.0;
    let (lat_mid, lon_mid) = corners
        .iter()
        .fold((0.0, 0.0), |(lat, lon), corner| (lat + corner.0 / 4.0, lon + unwound(corner.1) / 4.0));
    let quarter = |north: bool, west: bool| -> Vec<(f64, f64)> {
        corners
            .iter()
            .copied()
            .filter(|&(lat, lon)| (lat > lat_mid) == north && (unwound(lon) < lon_mid) == west)
            .collect()
    };
    match (
        quarter(true, true).as_slice(),
        quarter(true, false).as_slice(),
        quarter(false, false).as_slice(),
        quarter(false, true).as_slice(),
    ) {
        ([ul], [ur], [lr], [ll]) => [*ul, *ur, *lr, *ll],
        _ => corners,
    }
}
