use crate::module::components::last_segment;
use super::listing::{choice, Choice};
use super::browse::{BrowseState, Children};
use super::globe::GlobeState;
use super::layout::PaneId;
use super::listing::ListingState;
use super::preview::PreviewState;
use super::search::SearchState;

choice! {
    Shift {
        Up   = "up",   "Higher";
        Down = "down", "Lower";
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewId(pub(super) u64);

impl std::fmt::Display for ViewId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ViewId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(ViewId)
    }
}

pub enum ViewKind {
    Empty,
    Search(SearchState),
    Browse(BrowseState),
    Downloaded(ListingState),
    Preview(PreviewState),
    Globe(GlobeState),
    Shown,
}

impl ViewKind {
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

    pub fn listing_mut(&mut self) -> Option<&mut ListingState> {
        match self {
            ViewKind::Search(search) => Some(&mut search.listing),
            ViewKind::Browse(browse) => Some(&mut browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

    pub fn listing(&self) -> Option<&ListingState> {
        match self {
            ViewKind::Search(search) => Some(&search.listing),
            ViewKind::Browse(browse) => Some(&browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

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
    pub pane: PaneId,
}

const TITLE_LIMIT: usize = 22;

impl ViewKind {
    pub fn title(&self) -> String {
        match self {
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
