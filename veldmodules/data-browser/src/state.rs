pub mod types;
pub use types::*;

use veldsdk::Correlator;

pub enum Listing {
    Path(ViewId),
    Children(ViewId, String),
    Snapshot { product: String, files: u32, queued: usize },
}

pub enum Locate {
    Overlay(String),
    Outline(String),
}

pub enum Located {
    Asking,
    Missing,
    Failed,
    Found(crate::proto::data_provider::DataProduct),
}

#[derive(Default, PartialEq, Eq)]
pub enum Open {
    #[default]
    Nothing,
    TabMenu(PaneId),
    TabOptions(ViewId),
    Layer(String),
    Listing(ViewId, listing::Menu),
    Variables(ViewId),
    LayerVariables(String),
}

pub struct Highlight {
    pub key: String,
    pub view: Option<ViewId>,
    pub on_globe: bool,
}

pub const MIN_PANE: f32 = 180.0;

pub struct State {
    views: Vec<View>,
    layout: Layout,
    focus: PaneId,
    next_id: u64,
    pub start: crate::module::NewTab,
    pub last_saved: u64,
    pub library: library::LibraryState,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub window_surface: Option<veldsdk::surface::Delegated>,
    pub window: (u32, u32),
    pub scale: f32,
    pub open: Open,
    pub speed: f32,
    pub measured: Option<(i64, u64)>,
    pub outlined: Vec<globe::Outlined>,
    pub highlight: Option<Highlight>,
    pub located: std::collections::HashMap<String, Located>,
    pub overlays: Vec<overlay::OverlayState>,
    pub outlines: std::collections::HashSet<String>,
    pub showing: std::collections::HashSet<String>,
    pub file_wishes: std::collections::HashMap<String, String>,
    pub unshowable: std::collections::HashMap<String, String>,

    pub previews: Correlator<ViewId>,
    pub opens: Correlator<(String, crate::proto::globe::OverlayRole, u32, overlay::Part)>,
    pub imageries: Correlator<String>,
    pub preview_imagery: Correlator<ViewId>,
    pub listings: Correlator<Listing>,
    pub searches: Correlator<ViewId>,
    pub probe: veldsdk::Latest,
    pub locates: Correlator<Locate>,
    pub layout_writes: Correlator<u64>,
    pub layout_read: veldsdk::Latest,
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        let start = match config.initial_view.as_deref() {
            None | Some("search") => crate::module::NewTab::Search,
            Some("browse") => crate::module::NewTab::Browse,
            Some("downloaded") => crate::module::NewTab::Downloaded,
            Some(other) => {
                veldsdk::log::warn!(target: "system", "unknown initial_view '{}', falling back to Search", other);
                crate::module::NewTab::Search
            }
        };

        let layout = Layout::new();
        Ok(Self {
            views: Vec::new(),
            focus: layout.first(),
            layout,
            next_id: 1,
            start,
            last_saved: 0,
            library: library::LibraryState::default(),
            error: None,
            notice: None,
            unshowable: std::collections::HashMap::new(),
            window_surface: None,
            window: (0, 0),
            scale: 1.0,
            open: Open::Nothing,
            speed: 0.0,
            measured: None,
            outlined: Vec::new(),
            showing: std::collections::HashSet::new(),
            file_wishes: std::collections::HashMap::new(),
            outlines: std::collections::HashSet::new(),
            highlight: None,
            located: Default::default(),
            overlays: Vec::new(),
            previews: Correlator::new(),
            opens: Correlator::new(),
            imageries: Correlator::new(),
            preview_imagery: Correlator::new(),
            listings: Correlator::new(),
            searches: Correlator::new(),
            probe: veldsdk::Latest::default(),
            locates: Correlator::new(),
            layout_writes: Correlator::new(),
            layout_read: veldsdk::Latest::default(),
        })
    }

    pub fn split_pane(&mut self, pane: PaneId, side: Side, fraction: f32) -> Option<PaneId> {
        let fresh = self.layout.split(pane, side)?;
        if let Some(split) = self.layout.split_of(fresh) {
            self.layout.set_fraction(split, fraction);
            self.divide(split, 0.0);
        }
        Some(fresh)
    }

    pub fn open_in(&mut self, pane: PaneId, kind: ViewKind) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        let at = self.after_last(pane);
        self.views.insert(at, View { id, kind, pane });
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
        id
    }

    fn after_last(&self, pane: PaneId) -> usize {
        self.views
            .iter()
            .rposition(|view| view.pane == pane)
            .map_or(self.views.len(), |at| at + 1)
    }

    pub fn close(&mut self, id: ViewId) -> Option<View> {
        let at = self.views.iter().position(|view| view.id == id)?;
        let view = self.views.remove(at);
        if self.layout.active(view.pane) == Some(id) {
            self.layout.set_active(view.pane, self.neighbour(view.pane, at));
        }
        self.forget_if_empty(view.pane);
        Some(view)
    }

    pub fn forget_if_empty(&mut self, pane: PaneId) {
        if self.views_in(pane).next().is_some() {
            return;
        }
        if let Some(took) = self.layout.remove(pane)
            && self.focus == pane
        {
            self.focus = took;
        }
    }

    fn neighbour(&self, pane: PaneId, at: usize) -> Option<ViewId> {
        let right = self.views.iter().skip(at).find(|view| view.pane == pane);
        let left = self.views.iter().take(at).rev().find(|view| view.pane == pane);
        right.or(left).map(|view| view.id)
    }

    pub fn focus(&mut self, id: ViewId) {
        let Some(view) = self.views.iter().find(|view| view.id == id) else { return };
        let pane = view.pane;
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn collapse(&mut self) {
        let kept = self.layout.collapse(self.focus);
        for view in &mut self.views {
            view.pane = kept;
        }
        let active = self.layout.active(kept).or_else(|| self.views.first().map(|view| view.id));
        self.layout.set_active(kept, active);
        self.focus = kept;
    }

    pub fn move_to(&mut self, id: ViewId, pane: PaneId) {
        let Some(at) = self.views.iter().position(|view| view.id == id) else { return };
        let from = self.views[at].pane;
        if !self.layout.contains(pane) {
            return;
        }
        if from == pane {
            self.layout.set_active(pane, Some(id));
            self.focus = pane;
            return;
        }

        let mut view = self.views.remove(at);
        let left_behind = self.neighbour(from, at);
        view.pane = pane;
        let to = self.after_last(pane);
        self.views.insert(to, view);

        if self.layout.active(from) == Some(id) {
            self.layout.set_active(from, left_behind);
        }
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
        self.forget_if_empty(from);
    }

    pub fn move_aside(&mut self, id: ViewId, side: Side) {
        if !self.can_move(id, side) {
            return;
        }
        let Some(from) = self.pane_of(id) else { return };
        let target = match self.layout.neighbour(from, side) {
            Some(pane) => pane,
            None => match self.layout.split(from, side) {
                Some(pane) => pane,
                None => return,
            },
        };
        self.move_to(id, target);
    }

    pub fn drop_beside(&mut self, id: ViewId, pane: PaneId, side: Side) {
        if self.pane_of(id) == Some(pane) && self.views_in(pane).count() < 2 {
            return;
        }
        let Some(fresh) = self.split_pane(pane, side, 0.5) else { return };
        self.move_to(id, fresh);
    }

    pub fn can_move(&self, id: ViewId, side: Side) -> bool {
        let Some(from) = self.pane_of(id) else { return false };
        match self.layout.neighbour(from, side) {
            Some(_) => true,
            None => self.views_in(from).count() > 1,
        }
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    pub fn views_in(&self, pane: PaneId) -> impl Iterator<Item = &View> {
        self.views.iter().filter(move |view| view.pane == pane)
    }

    pub fn active_in(&self, pane: PaneId) -> Option<ViewId> {
        self.layout.active(pane)
    }

    pub fn pane_of(&self, id: ViewId) -> Option<PaneId> {
        self.views.iter().find(|view| view.id == id).map(|view| view.pane)
    }

    pub fn get(&self, id: ViewId) -> Option<&ViewKind> {
        self.views.iter().find(|view| view.id == id).map(|view| &view.kind)
    }

    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut ViewKind> {
        self.views.iter_mut().find(|view| view.id == id).map(|view| &mut view.kind)
    }

    pub fn find(&self, matches: impl Fn(&ViewKind) -> bool) -> Option<ViewId> {
        self.views.iter().find(|view| matches(&view.kind)).map(|view| view.id)
    }

    pub fn logical_width(&self) -> f32 {
        self.window.0 as f32 / self.scale.max(1.0)
    }

    fn layout_height(&self) -> f32 {
        let chrome = crate::module::theme::BAR_HEIGHT + crate::module::theme::HAIRLINE;
        (self.window.1 as f32 / self.scale.max(1.0) - chrome).max(0.0)
    }

    pub fn divide(&mut self, split: SplitId, delta: f32) {
        let Some(axis) = self.layout.axis_of(split) else { return };
        let (wide, tall) = self.layout.split_share(split);
        let room = match axis {
            Axis::Row => self.logical_width() * wide,
            Axis::Column => self.layout_height() * tall,
        };
        if room <= 0.0 {
            return;
        }
        self.layout.nudge(split, delta / room, MIN_PANE / room);
    }

    pub fn pane_width(&self, id: ViewId) -> f32 {
        let share = self.pane_of(id).map_or(1.0, |pane| self.layout.share(pane).0);
        self.logical_width() * share
    }

    pub fn listing_mut(&mut self, id: ViewId) -> Option<&mut listing::ListingState> {
        self.get_mut(id)?.listing_mut()
    }

    pub fn close_menus(&mut self) {
        self.open = Open::Nothing;
    }

    pub fn toggle_open(&mut self, what: Open) {
        self.open = match self.open == what {
            true => Open::Nothing,
            false => what,
        };
    }

    pub fn menu_in(&self, view: ViewId) -> Option<&listing::Menu> {
        match &self.open {
            Open::Listing(id, menu) if *id == view => Some(menu),
            _ => None,
        }
    }

    pub fn layer_menu(&self, key: &str) -> bool {
        matches!(&self.open, Open::Layer(open) if open == key)
    }

    pub fn variables_menu(&self, view: ViewId) -> bool {
        matches!(&self.open, Open::Variables(open) if *open == view)
    }

    pub fn layer_variables(&self, key: &str) -> bool {
        matches!(&self.open, Open::LayerVariables(open) if open == key)
    }

    pub fn refine(&mut self, view: ViewId) {
        self.close_menus();
        if let Some(listing) = self.listing_mut(view) {
            listing.page = 0;
        }
    }

    pub fn picked_key(&self) -> &str {
        self.highlight.as_ref().filter(|it| it.on_globe).map_or("", |it| it.key.as_str())
    }

    pub fn target_in(&self, view: ViewId) -> &str {
        self.highlight
            .as_ref()
            .filter(|it| it.view == Some(view))
            .map_or("", |it| it.key.as_str())
    }

    pub fn deselect(&mut self) -> bool {
        let Some(highlight) = self.highlight.as_mut().filter(|it| it.on_globe) else {
            return false;
        };
        highlight.on_globe = false;
        if highlight.view.is_none() {
            self.highlight = None;
        }
        true
    }

    pub fn drop_target_in(&mut self, view: ViewId) {
        let Some(highlight) = self.highlight.as_mut().filter(|it| it.view == Some(view)) else {
            return;
        };
        highlight.view = None;
        if !highlight.on_globe {
            self.highlight = None;
        }
    }

    pub fn browse_mut(&mut self, id: ViewId) -> Option<&mut browse::BrowseState> {
        match self.get_mut(id)? {
            ViewKind::Browse(browse) => Some(browse),
            _ => None,
        }
    }

    pub fn search_mut(&mut self, id: ViewId) -> Option<&mut search::SearchState> {
        match self.get_mut(id)? {
            ViewKind::Search(search) => Some(search),
            _ => None,
        }
    }

    pub fn globe_mut(&mut self, id: ViewId) -> Option<&mut globe::GlobeState> {
        match self.get_mut(id)? {
            ViewKind::Globe(globe) => Some(globe),
            _ => None,
        }
    }

    pub fn preview_mut(&mut self, id: ViewId) -> Option<&mut preview::PreviewState> {
        match self.get_mut(id)? {
            ViewKind::Preview(preview) => Some(preview),
            _ => None,
        }
    }

    pub fn picked(&self) -> Option<&globe::Outlined> {
        let key = self.picked_key();
        self.outlined.iter().find(|outlined| outlined.key == key)
    }

}

pub struct Batch {
    pub deletable: bool,
    pub fetchable: bool,
}

pub mod search;
pub mod browse;
pub mod globe;
pub mod layout;
pub mod library;
pub mod listing;
pub mod overlay;
pub mod preview;

pub use layout::{Axis, Layout, Node, PaneId, Side, SplitId};

pub use browse::BrowseState;
pub use globe::GlobeState;
pub use library::LibraryState;
pub use preview::PreviewState;
pub use search::SearchState;
