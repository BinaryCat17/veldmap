//! components/rows.rs — list rows built from what the view lives by.
//!
//! Separate from the layout because the layout is not the only one asking for
//! them: "select all" and jump-to-row count by the same rows that are shown —
//! and a second assembly would drift from the first exactly where it is most
//! visible, in the selection.
//!
//! Knowledge of views here is exactly enough to name the source: the catalogue
//! answers with listing records, search with products, downloaded with library
//! records. Everything else about a row lives in `row`.

use crate::module::components::row::{
    bare, downloaded_rows, folder_path, OnGlobe, OnOutline, Row, RowKind, RowStatus,
};
use crate::module::state::browse::{BrowseItem, Children};
use crate::module::state::listing::ListingState;
use crate::module::state::{BrowseState, Located, SearchState, State, ViewId, ViewKind};

/// Rows of the named view. Empty — the view is not list-like: preview, globe,
/// and "Shown" have no table at all.
pub fn of(state: &State, view: ViewId) -> Vec<Row> {
    let Some(kind) = state.get(view) else { return Vec::new() };
    match kind {
        ViewKind::Browse(opened) => browse(state, opened),
        ViewKind::Search(opened) => search(state, opened),
        ViewKind::Downloaded(_) => {
            // Downloaded is built from library records, not catalogue keys —
            // it has no shared row assembly (`from_key`), and the globe mark is
            // set here. Depth is walked with the same traversal as the shared
            // assembly: an identical-looking subtree must not light up in the
            // catalogue and stay silent here.
            let mut rows = downloaded_rows(&state.library);
            mark_globe(state, &mut rows);
            rows
        }
        // They have no table at all; named one by one so that a new view with
        // a list does not silently end up without rows.
        ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => {
            Vec::new()
        }
    }
}

/// Set the globe mark on the rows of "Downloaded" and everything under them.
fn mark_globe(state: &State, rows: &mut [Row]) {
    for row in rows {
        row.globe = onto_globe(state, row.product_key());
        row.outlined = outlined(state, row.product_key());
        row.unshowable = unshowable(state, row.product_key());
        mark_globe(state, &mut row.children);
    }
}

/// Why the display of this snapshot did not come out; empty — not attempted or
/// it worked.
///
/// By the snapshot key, as with everything about the globe: the reason belongs
/// to the snapshot, not the row, and the same snapshot in the catalogue, in the
/// results, and in downloaded must be explained the same way.
pub fn unshowable(state: &State, key: &str) -> String {
    state.unshowable.get(key).cloned().unwrap_or_default()
}

/// How the snapshot of this row lies on the globe (see [`OnGlobe`]).
///
/// Asked by the snapshot key, not the row key: a catalogue row knows the folder
/// with a trailing slash, while the overlay remembers the product without it.
///
/// The only place where the snapshot key meets the set of overlays. Besides the
/// icon and the progress bar, the occupied-area hatching asks it too (see
/// `handlers::outline::send`): all three ask one question — "what is happening
/// with this snapshot on the globe now" — and three answers to it would drift
/// apart silently.
pub fn onto_globe(state: &State, key: &str) -> OnGlobe {
    let Some(overlay) = state.overlays.iter().find(|overlay| overlay.identifier == key) else {
        // The overlay is not there yet, but the display is already requested:
        // the product by key is restored by the catalogue, and reaching it is
        // network work. These seconds must not pass in silence — the click
        // would look lost, and a second click on the same icon is exactly what
        // withdraws the request.
        return match state.showing.contains(key) {
            true => OnGlobe::Asked,
            false => OnGlobe::Off,
        };
    };
    match overlay.on_globe() {
        false => OnGlobe::Assembling,
        true => OnGlobe::Laid { hidden: overlay.hidden, progress: overlay.progress },
    }
}

/// Whether the snapshot of this row is outlined (see [`OnOutline`]).
///
/// Requested and drawn are different things: the geometry is known to the
/// catalogue, reaching it is network work, and seconds pass between the click
/// and the outline. The icon must light up at once, otherwise the click looks
/// lost.
fn outlined(state: &State, key: &str) -> OnOutline {
    if key.is_empty() || !state.outlines.contains(key) {
        return OnOutline::Off;
    }
    if state.outlined.iter().any(|outlined| outlined.key == key) {
        return OnOutline::Drawn;
    }
    match state.located.get(key) {
        // No answer yet: the request goes out with the same rebuild as this
        // row.
        None | Some(Located::Asking) => OnOutline::Asking,
        // Asking failed — that is about the network, not the snapshot, and the
        // same icon can ask again.
        Some(Located::Failed) => OnOutline::Failed,
        // The catalogue answered but there is nothing to draw: it either does
        // not know this key or knows a product without geometry.
        Some(Located::Missing) => OnOutline::Blank,
        // Geometry is known: empty — nothing to draw, non-empty — already
        // drawn. It can disagree with `state.outlined` only within one rebuild
        // that puts both there and here.
        Some(Located::Found(found)) => match found.footprint.is_empty() {
            true => OnOutline::Blank,
            false => OnOutline::Drawn,
        },
    }
}

/// Network catalogue: records of the current folder and the contents of the
/// expanded ones.
pub fn browse(state: &State, view: &BrowseState) -> Vec<Row> {
    entries(state, &view.items, &view.children, &view.listing)
}

/// Search results: each row is a snapshot, and an expanded one shows its
/// contents with the same listing as the catalogue.
pub fn search(state: &State, view: &SearchState) -> Vec<Row> {
    view.results
        .iter()
        .map(|product| {
            // Rows are told apart only by how the product lies in storage (see
            // DataProduct.folder): a GET on a folder-product path is a 404, and
            // "opening" it means going inside, with the same rows as folders of
            // the network catalogue; an archive-product is an ordinary object.
            let kind = RowKind::Product { folder: product.folder };
            let mut row = Row {
                product_type: product.product_type.clone(),
                unviewable: product.unviewable.clone(),
                ..from_key(
                    state,
                    product.identifier.clone(),
                    product.name.clone(),
                    product.size,
                    product.acquired,
                    kind,
                    // In the results the row is the snapshot — its own product.
                    product.identifier.clone(),
                )
            };
            match product.parts.len() {
                // One part — the snapshot is that part, and it expands
                // straight into its files.
                0 | 1 => fill(state, &mut row, &view.children, &view.listing),
                _ => parted(state, &mut row, product, &view.children, &view.listing),
            }
            row
        })
        .collect()
}

/// A snapshot the catalogue gave several products for: it expands into its
/// parts, and each of them into its files.
///
/// A tier higher, because a part is not a file of the snapshot but the shot
/// itself in another form: another processing of the same (receiver raw,
/// striped TIFF, tiled COG) or another measured quantity of the same pass
/// (nitrogen dioxide, carbon monoxide, ozone). Laying them out mixed with the
/// files would mean saying that the snapshot consists of them.
fn parted(
    state: &State,
    row: &mut Row,
    product: &crate::proto::data_provider::DataProduct,
    children: &Children,
    listing: &ListingState,
) {
    // Its own key, because the shown part stands under the snapshot with a row
    // of its own: with one key for both, both would expand at once.
    row.group = Some(crate::module::components::row::scene_key(&product.identifier));
    if !listing.expanded.contains(row.key()) {
        return;
    }
    row.children = product
        .parts
        .iter()
        .map(|part| {
            let mut child = Row {
                // The part is captioned by its type, not the file name: names
                // of parts of one shot differ only in the tail, and rows with
                // such names one under another say nothing.
                title: match part.product_type.is_empty() {
                    true => part.name.clone(),
                    false => part.product_type.clone(),
                },
                // The view column is free for a part — its type is already the
                // caption — and it holds what one needs to know about it:
                // whether it is shown or kept in reserve. This does not fit the
                // title: the row name is squeezed to the column width, and the
                // mark would be the first to shrink.
                product_type: match part.shown {
                    true => "shown".to_string(),
                    false => "part".to_string(),
                },
                unviewable: part.unviewable.clone(),
                ..from_key(
                    state,
                    part.identifier.clone(),
                    part.name.clone(),
                    part.size,
                    product.acquired,
                    RowKind::Product { folder: part.folder },
                    part.identifier.clone(),
                )
            };
            fill(state, &mut child, children, listing);
            child
        })
        .collect();
}

/// Listing records → rows together with the contents of expanded folders.
fn entries(
    state: &State,
    items: &[BrowseItem],
    children: &Children,
    listing: &ListingState,
) -> Vec<Row> {
    items
        .iter()
        .map(|item| {
            // The provider makes a record a snapshot: only it knows the bucket
            // layout (see `ListEntry.product`). It can still remain a folder —
            // a .SAFE is a folder — and nobody takes away going inside.
            let itself = item.product == bare(&item.identifier);
            let kind = match (itself, item.is_folder) {
                (true, folder) => RowKind::Product { folder },
                (false, true) => RowKind::Folder,
                (false, false) => RowKind::File,
            };
            let mut row = Row {
                unviewable: item.unviewable.clone(),
                ..from_key(
                    state,
                    item.identifier.clone(),
                    item.name.clone(),
                    item.size,
                    item.modified,
                    kind,
                    item.product.clone(),
                )
            };
            fill(state, &mut row, children, listing);
            row
        })
        .collect()
}

/// A row from a storage key — one assembly for both the catalogue and the
/// search results.
///
/// One question separates them: is there content inside. What contains
/// something else has neither size nor time of its own — in S3 a folder is a
/// shared key prefix; but how much of its content is already on disk is known,
/// and it is the library, not the catalogue, that knows it. An ordinary object
/// is the other way around: size and time were told by the catalogue, and the
/// state — by the library, if it has a record.
fn from_key(
    state: &State,
    identifier: String,
    title: String,
    size: u64,
    date: i64,
    kind: RowKind,
    product: String,
) -> Row {
    // Both globe states are here because rows of the catalogue and of the
    // search results are assembled here: the snapshot in them is the same, and
    // two answers to these questions would drift apart one day.
    //
    // They are asked by the row's snapshot, not the row itself: the snapshot is
    // what lies on the globe, and a file inside it answers about it, not about
    // itself (see [`Row::product_key`]). This gives the file no bar of its own
    // — bars are drawn only for snapshots — but the menu item that puts a
    // snapshot on the globe knows whether it is already there.
    let snapshot = match product.is_empty() {
        false => bare(&product),
        true => bare(&identifier),
    };
    let globe = onto_globe(state, snapshot);
    let outlined = outlined(state, snapshot);
    let unshowable = unshowable(state, snapshot);
    if kind.is_folder() {
        // Two different questions about a folder, and the second is asked only
        // of a snapshot. How much of its content is on disk is seen from the
        // record keys — and that is all one can say about a folder path: how
        // many files should be in it, nobody knows. A snapshot has that number
        // (`LibraryEntry.siblings`), and only with it "all on disk" is a
        // statement, not a guess.
        //
        // It is asked by the snapshot, not the row key: a folder key arrives
        // with a trailing slash, while library records remember the snapshot
        // without it.
        let under = state.library.under(&folder_path(&identifier));
        let whole = state.library.whole(&product);
        // As long as one file is moving, the whole snapshot is moving — the
        // same rule as the folded row of "Downloaded" (see `row::snapshot`).
        // Without it a downloading snapshot stands with a green "3 on disk" and
        // no bar: two different answers to one question, and they would drift
        // apart silently.
        //
        // A snapshot, not just any folder: a path folder has no download of its
        // own, and what downloads inside it is the snapshot within. Calling it
        // "downloading" would take away its file count on disk — the only thing
        // that can be said about a folder at all — and give nothing in return:
        // it has no size either, and the bar has nothing to build from.
        let downloading = under.active && kind.is_product();
        let status = match (whole, downloading, under.files) {
            (true, ..) => RowStatus::Complete,
            // Total bytes are what the catalogue said about size: the library
            // itself knows only what is already downloaded, and without a
            // denominator the bar has nothing to build from.
            (false, true, _) => RowStatus::Downloading { done: under.bytes, total: size },
            (false, false, 0) => RowStatus::Remote,
            // A folder has nothing to stand on: it has no downloads of its own.
            (false, false, done) => RowStatus::Partial { done, trouble: String::new() },
        };
        let row = Row::container_row(identifier, title, status, kind);
        return Row { size, date, product, globe, outlined, unshowable, ..row };
    }
    Row {
        product,
        globe,
        outlined,
        unshowable,
        ..Row::remote(&state.library, identifier, title, size, date, kind)
    }
}

/// The contents of an expanded folder as sub-rows under it, and so deeper: a
/// folder expanded inside an expanded one is ordinary business, and there is no
/// second rule for it.
///
/// An unexpanded one stays silent about its contents even when they have
/// already arrived: the row shows what was expanded, not what managed to load.
fn fill(state: &State, row: &mut Row, children: &Children, listing: &ListingState) {
    if !row.kind.is_folder() || !listing.expanded.contains(row.key()) {
        return;
    }
    let path = folder_path(&row.identifier);
    row.loading = children.waiting(&path);
    row.children = entries(state, children.get(&path), children, listing);
}
