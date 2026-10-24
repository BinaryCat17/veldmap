//! A view — what is shown in one tab.
//!
//! The `ViewKind` variant itself holds the state, not the module: two open
//! Browse views are two independent folders, not one screen with a shared
//! variable.

use crate::module::components::last_segment;
use super::listing::{choice, Choice};
use super::browse::{BrowseState, Children};
use super::globe::GlobeState;
use super::layout::PaneId;
use super::listing::ListingState;
use super::preview::PreviewState;
use super::search::SearchState;

choice! {
    /// Which way to move a layer in the set. An enum, not "up: yes/no": a shift
    /// has two equal directions, and one is not the negation of the other — it
    /// is simply different.
    Shift {
        Up   = "up",   "Higher";
        Down = "down", "Lower";
    }
}

/// Identifier of an open view. Unique within a session and never reused: it
/// addresses both the tab in the markup and the reply to this view's request —
/// and the view may already be closed by then, and a reused id would hand a
/// foreign resource to a live tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewId(pub(super) u64);

impl std::fmt::Display for ViewId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A tab returns its id as a string in `UiEventResponse.value` — parsing it
/// back happens where the click is handled.
impl std::str::FromStr for ViewId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(ViewId)
    }
}

pub enum ViewKind {
    /// Created but showing nothing yet: it asks what to fill it with. It has no
    /// state of its own — its whole appearance is that question.
    ///
    /// A tab, not an empty pane: a pane without tabs does not live in the tree
    /// (see `state::layout`), while a place where the person has not decided
    /// what to view yet is needed — every "open beside" starts from it.
    Empty,
    Search(SearchState),
    Browse(BrowseState),
    /// Rows come from `LibraryState` — there is one per module and it arrives
    /// by broadcast (see state::library); only how to show them is local here.
    Downloaded(ListingState),
    Preview(PreviewState),
    /// The globe is drawn by a separate module; here there is only room for it
    /// and the gestures.
    Globe(GlobeState),
    /// What currently lies on the globe. The view has no state of its own: the
    /// layers live in `State::overlays` — they are a property of the globe, not
    /// of the screen, and outlive this tab's closing, as its outlines do.
    Shown,
}

impl ViewKind {
    /// Which tab kind this view is opened as. `None` — snapshot preview: it is
    /// not opened from the menu, a list row opens it.
    ///
    /// Needed so that the label and the kind icon live in one place: a tab and
    /// a menu item denote the same thing, and diverging icons would read as two
    /// different things.
    pub fn opened_as(&self) -> Option<crate::module::NewTab> {
        use crate::module::NewTab;
        Some(match self {
            ViewKind::Empty => NewTab::Empty,
            ViewKind::Search(_) => NewTab::Search,
            ViewKind::Browse(_) => NewTab::Browse,
            ViewKind::Downloaded(_) => NewTab::Downloaded,
            ViewKind::Globe(_) => NewTab::Globe,
            ViewKind::Shown => NewTab::Shown,
            ViewKind::Preview(_) => return None,
        })
    }

    /// List display settings — every view except the preview has them, and the
    /// same messages edit them. Without this each handler would destructure
    /// `ViewKind` again, in its own way and with three branches.
    pub fn listing_mut(&mut self) -> Option<&mut ListingState> {
        match self {
            ViewKind::Search(search) => Some(&mut search.listing),
            ViewKind::Browse(browse) => Some(&mut browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

    /// The same, read-only — outline assembly asks them: what is selected in
    /// this list (see handlers::outline).
    pub fn listing(&self) -> Option<&ListingState> {
        match self {
            ViewKind::Search(search) => Some(&search.listing),
            ViewKind::Browse(browse) => Some(&browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

    /// Content of expanded folders. Present where rows come from the provider's
    /// catalogue: for downloaded, the snapshot's files are already at hand and
    /// there is nothing to page through.
    pub fn children_mut(&mut self) -> Option<&mut Children> {
        match self {
            ViewKind::Search(search) => Some(&mut search.children),
            ViewKind::Browse(browse) => Some(&mut browse.children),
            ViewKind::Empty
            | ViewKind::Downloaded(_)
            | ViewKind::Preview(_)
            | ViewKind::Globe(_)
            | ViewKind::Shown => None,
        }
    }
}

pub struct View {
    pub id: ViewId,
    pub kind: ViewKind,
    /// Which pane this tab lies in. A field on the view, not a tab list on the
    /// pane: the id is unique across the window, and a lookup by it must have
    /// one answer — while a list on each pane would answer by scanning all of
    /// them, and would also allow a tab in two places at once.
    pub pane: PaneId,
}

/// How many title characters fit on a tab. Copernicus product names reach
/// eighty characters, and a tab as wide as the name means one tab per window;
/// the full name is visible in the view's own header.
const TITLE_LIMIT: usize = 22;

impl ViewKind {
    /// The tab title — the last path segment, truncated to `TITLE_LIMIT`.
    pub fn title(&self) -> String {
        match self {
            // The empty tab's label matches what the "plus" offers for it:
            // it is the same thing before and after the click.
            ViewKind::Empty => crate::module::NewTab::Empty.title().to_string(),
            ViewKind::Search(_) => "Snapshot search".to_string(),
            ViewKind::Browse(browse) => match last_segment(&browse.current_path) {
                "" => "Catalogue".to_string(),
                name => ellipsize(name),
            },
            ViewKind::Downloaded(_) => "Downloaded".to_string(),
            ViewKind::Preview(preview) => match last_segment(&preview.label) {
                "" => "Preview".to_string(),
                name => ellipsize(name),
            },
            ViewKind::Globe(_) => "Globe".to_string(),
            ViewKind::Shown => "On view".to_string(),
        }
    }
}

fn ellipsize(name: &str) -> String {
    crate::module::components::format::ellipsize(name, TITLE_LIMIT)
}
