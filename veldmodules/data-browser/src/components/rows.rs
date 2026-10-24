use crate::module::components::row::{
    bare, downloaded_rows, folder_path, OnGlobe, OnOutline, Row, RowKind, RowStatus,
};
use crate::module::state::browse::{BrowseItem, Children};
use crate::module::state::listing::ListingState;
use crate::module::state::{BrowseState, Located, SearchState, State, ViewId, ViewKind};

pub fn of(state: &State, view: ViewId) -> Vec<Row> {
    let Some(kind) = state.get(view) else { return Vec::new() };
    match kind {
        ViewKind::Browse(opened) => browse(state, opened),
        ViewKind::Search(opened) => search(state, opened),
        ViewKind::Downloaded(_) => {
            let mut rows = downloaded_rows(&state.library);
            mark_globe(state, &mut rows);
            rows
        }
        ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => {
            Vec::new()
        }
    }
}

fn mark_globe(state: &State, rows: &mut [Row]) {
    for row in rows {
        row.globe = onto_globe(state, row.product_key());
        row.outlined = outlined(state, row.product_key());
        row.unshowable = unshowable(state, row.product_key());
        mark_globe(state, &mut row.children);
    }
}

pub fn unshowable(state: &State, key: &str) -> String {
    state.unshowable.get(key).cloned().unwrap_or_default()
}

pub fn onto_globe(state: &State, key: &str) -> OnGlobe {
    let Some(overlay) = state.overlays.iter().find(|overlay| overlay.identifier == key) else {
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

fn outlined(state: &State, key: &str) -> OnOutline {
    if key.is_empty() || !state.outlines.contains(key) {
        return OnOutline::Off;
    }
    if state.outlined.iter().any(|outlined| outlined.key == key) {
        return OnOutline::Drawn;
    }
    match state.located.get(key) {
        None | Some(Located::Asking) => OnOutline::Asking,
        Some(Located::Failed) => OnOutline::Failed,
        Some(Located::Missing) => OnOutline::Blank,
        Some(Located::Found(found)) => match found.footprint.is_empty() {
            true => OnOutline::Blank,
            false => OnOutline::Drawn,
        },
    }
}

pub fn browse(state: &State, view: &BrowseState) -> Vec<Row> {
    entries(state, &view.items, &view.children, &view.listing)
}

pub fn search(state: &State, view: &SearchState) -> Vec<Row> {
    view.results
        .iter()
        .map(|product| {
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
                    product.identifier.clone(),
                )
            };
            match product.parts.len() {
                0 | 1 => fill(state, &mut row, &view.children, &view.listing),
                _ => parted(state, &mut row, product, &view.children, &view.listing),
            }
            row
        })
        .collect()
}

fn parted(
    state: &State,
    row: &mut Row,
    product: &crate::proto::data_provider::DataProduct,
    children: &Children,
    listing: &ListingState,
) {
    row.group = Some(crate::module::components::row::scene_key(&product.identifier));
    if !listing.expanded.contains(row.key()) {
        return;
    }
    row.children = product
        .parts
        .iter()
        .map(|part| {
            let mut child = Row {
                title: match part.product_type.is_empty() {
                    true => part.name.clone(),
                    false => part.product_type.clone(),
                },
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

fn entries(
    state: &State,
    items: &[BrowseItem],
    children: &Children,
    listing: &ListingState,
) -> Vec<Row> {
    items
        .iter()
        .map(|item| {
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

fn from_key(
    state: &State,
    identifier: String,
    title: String,
    size: u64,
    date: i64,
    kind: RowKind,
    product: String,
) -> Row {
    let snapshot = match product.is_empty() {
        false => bare(&product),
        true => bare(&identifier),
    };
    let globe = onto_globe(state, snapshot);
    let outlined = outlined(state, snapshot);
    let unshowable = unshowable(state, snapshot);
    if kind.is_folder() {
        let under = state.library.under(&folder_path(&identifier));
        let whole = state.library.whole(&product);
        let downloading = under.active && kind.is_product();
        let status = match (whole, downloading, under.files) {
            (true, ..) => RowStatus::Complete,
            (false, true, _) => RowStatus::Downloading { done: under.bytes, total: size },
            (false, false, 0) => RowStatus::Remote,
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

fn fill(state: &State, row: &mut Row, children: &Children, listing: &ListingState) {
    if !row.kind.is_folder() || !listing.expanded.contains(row.key()) {
        return;
    }
    let path = folder_path(&row.identifier);
    row.loading = children.waiting(&path);
    row.children = entries(state, children.get(&path), children, listing);
}
