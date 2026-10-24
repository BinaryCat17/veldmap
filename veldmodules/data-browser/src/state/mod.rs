pub mod types;
pub use types::*;

use veldsdk::Correlator;

/// Who receives the data-provider/on_list_path reply. The topic is one, but
/// three callers ask it, and "whose id is this" must have exactly one answer —
/// iterating over views cannot answer it.
pub enum Listing {
    /// Contents of the folder the tab shows.
    Path(ViewId),
    /// Contents of an expanded row: the tab and the key of its folder.
    Children(ViewId, String),
    /// All files of a snapshot — for download as a whole. There is no tab here:
    /// downloads go to the library, and there is one library per window. The
    /// counters travel with the request instead of living in state: several
    /// snapshots can download at once, and a shared counter would break the
    /// second one halfway through the first.
    ///
    /// `files` counts how many snapshot files passed by us, `queued` counts how
    /// many of them were put into download. The first is counted for the
    /// library: only this walk can traverse the snapshot in storage, and its
    /// result is the only answer to "how many files there should be".
    Snapshot { product: String, files: u32, queued: usize },
}

/// Who receives the data-provider/on_locate_result reply. Two callers wait for
/// it, and their contents cannot tell them apart: the product is the same in
/// both cases.
pub enum Locate {
    /// Put the snapshot on the globe — the globe icon in a row was pressed. The
    /// same call also fills the geometry cache: the outline of this snapshot
    /// would ask the catalogue for the same thing (see
    /// `handlers::overlay::on_toggle_pressed`).
    Overlay(String),
    /// Geometry of the marked snapshot: to outline it. The key is the one by
    /// which it is marked in the list; it is also the cache key (see
    /// `State::located`).
    Outline(String),
}

/// What is known about a product by row key.
///
/// One table for all three states, not an "answer cache" plus "asked": this is
/// one piece of knowledge in three positions, and two sets for it would be held
/// by the convention "a key is never in both" — a convention that will be
/// broken one day.
pub enum Located {
    /// Asked, no answer yet. Asking again is pointless: outline assembly runs on
    /// every mark, and without this one key would travel to the catalogue a
    /// dozen times.
    Asking,
    /// The catalogue does not know it. That is an answer too, and it must be
    /// remembered: otherwise the not-found would be asked forever.
    Missing,
    /// The ask failed — the network or a service refusal. This says nothing
    /// about the snapshot itself, so the mark is temporary: the next mark of
    /// the same row clears it, and "click again" is exactly the retry. It does
    /// not repeat on its own — otherwise an answer to a failed request would
    /// call the next one, and so on until the end of the run.
    Failed,
    Found(crate::proto::data_provider::DataProduct),
}

/// What is expanded on screen right now.
///
/// One field for the whole screen, not a field per carrier: only one thing can
/// be expanded, and the rule is about the screen, not about a pane, tab, or
/// layer. Spread across carriers, it would be held by assignments in every
/// handler — and forgetting one of them means leaving two expanded menus, and
/// only eyes can notice that.
#[derive(Default, PartialEq, Eq)]
pub enum Open {
    #[default]
    Nothing,
    /// The "plus" of the tab strip of the named pane.
    TabMenu(PaneId),
    /// The menu of the tab itself: where to put it.
    TabOptions(ViewId),
    /// The menu of a "Now viewing" layer. The layer is named by the snapshot
    /// key — the same key addresses it everywhere else.
    Layer(String),
    /// A menu inside the list of the named tab: a strip chip or a row
    /// (see [`listing::Menu`]).
    Listing(ViewId, listing::Menu),
    /// The variable list of the file under the preview frame of the named tab.
    Variables(ViewId),
    /// The variable list of the detailed file in the row of the "Now viewing"
    /// layer named by the snapshot key.
    LayerVariables(String),
}

/// What is the main thing on screen right now — one snapshot for the whole
/// screen.
///
/// Two things set the highlight: a jump to a row and a click on an outline on
/// the globe. They ask one question, "what is the main thing here", and it has
/// no two answers — hence one field. Spread across carriers, the highlight
/// would be held by clearing from every handler, and both rows would stay lit:
/// the one we were led to half an hour ago and the one chosen now.
pub struct Highlight {
    /// The snapshot key. By it its row is lit in every list where it exists:
    /// marking happens in one, but it must be visible everywhere.
    pub key: String,
    /// The tab whose jump led to this row: in it the row is also brought to its
    /// page and scrolled into view. `None` means the globe set the highlight and
    /// there is no tab.
    pub view: Option<ViewId>,
    /// Whether the snapshot is ringed on the globe by the ribbon. A click on an
    /// outline sets it; a jump to a row does not light the ribbon — the globe is
    /// not being looked at at that moment.
    pub on_globe: bool,
}

/// How narrow a pane may be made, in layout points. Narrower than this, neither
/// the tab strip nor its contents is visible in it.
pub const MIN_PANE: f32 = 180.0;

pub struct State {
    /// Open views in tab order. Order is a property of the layout, not of the
    /// set: closing a neighbor must not change where the rest sit. The pane a
    /// tab lies in is its own field (see `View::pane`), and the order within a
    /// pane is the order of this same list.
    views: Vec<View>,
    /// How the screen is composed: a tree of panes with their shares and active
    /// tabs (see state::layout).
    layout: Layout,
    /// The pane whose active tab receives widget events. An event comes from a
    /// widget, not from a pane, and not all widgets can name themselves — so the
    /// screen has one pane that is "at hand right now".
    focus: PaneId,
    next_id: u64,
    /// Where to start when there is nothing to recall — from the config. A saved
    /// layout wins: the config says where the first run starts, not what to show
    /// instead of where the person stopped.
    pub start: crate::module::NewTab,
    /// Fingerprint of the last written layout. It shows whether anything has
    /// changed since: the layout is written on every tab action, while most
    /// actions do not touch it (see handlers::persist).
    pub last_saved: u64,
    /// Cache of the library state — one per module: it arrives by broadcast and
    /// all views need it at once (see state::library).
    pub library: library::LibraryState,
    /// A library failure. The library sets and clears it itself: it broadcasts
    /// state on every change, and "no error" in that broadcast means "the
    /// complaint is gone". Each view shows the progress of its own work: there
    /// is one status line and many views, and "12 items" from a background tab
    /// under the active one's title would be a lie.
    pub error: Option<String>,
    /// What failed on a press: a snapshot without rasters, a snapshot clipped by
    /// the ceiling, a product not found in the catalogue.
    ///
    /// Separate from the library failure, not in the same field: the library
    /// broadcasts state for every queued file and clears its own message with it
    /// — and the work the notice is about is exactly what queued those files. In
    /// a shared field it would be erased before anyone could read it, and not by
    /// accident but always.
    ///
    /// It lives until the next action, which also clears it (see
    /// `module::on_ui_event`). It has no "close" of its own — a notice about what
    /// just failed must not outlive the next press.
    pub notice: Option<String>,
    /// Render target of our window: allocated in response to
    /// app/window_resized and delegated to the renderer (see handlers::window).
    pub window_surface: Option<veldsdk::surface::Delegated>,
    /// Window size in physical pixels (app/window_resized). Serves as the
    /// ceiling for previews: drawing a picture larger than the window is
    /// pointless.
    pub window: (u32, u32),
    /// Interface scale from the same place. With the size it gives the window
    /// width in layout points — by it the number of name characters that fit
    /// into their column is computed (see components::table).
    pub scale: f32,
    /// What is expanded on screen — one for all (see [`Open`]).
    pub open: Open,
    /// Download speed in bytes per second: derived from two adjacent library
    /// states (see handlers::library). The library has no field for it — it
    /// broadcasts what is there now, not how fast it grew.
    pub speed: f32,
    /// The previous measurement: when and how much was downloaded.
    pub measured: Option<(i64, u64)>,
    /// What is outlined on the globe right now. Not in the globe tab's state:
    /// outlines are a property of what is marked in the lists, not of the
    /// screen, and they travel to the renderer regardless of whether the tab is
    /// open (see handlers::outline).
    ///
    /// Gathered here from the batch selection of all lists at once — hence not
    /// in a view's state: there is one globe, and any number of lists that mark
    /// snapshots on it.
    pub outlined: Vec<globe::Outlined>,
    /// What is highlighted on screen (see [`Highlight`]).
    pub highlight: Option<Highlight>,
    /// Catalogue product by row key: what was asked and what was answered (see
    /// [`Located`]). Search rows have the product at hand and never get here.
    pub located: std::collections::HashMap<String, Located>,
    /// Snapshot overlays on the globe — what is being assembled or already shown
    /// (see handlers::overlay). The order is the layer order bottom to top, the
    /// same the globe understands; the "Now viewing" list reverses it itself,
    /// because "new ones on top" is a property of the screen, not of the set.
    pub overlays: Vec<overlay::OverlayState>,
    /// Snapshots asked to be outlined on the globe.
    ///
    /// Its own set, not derived from selection in the lists: an outline, a
    /// raster show, and a row selection are three independent things, and merged
    /// into one they would answer for each other. One per application, not per
    /// tab: there is one globe, and "the outlines of this tab" does not answer
    /// the question "what is on the globe".
    ///
    /// This is a request, not a drawing: the geometry is known to the catalogue,
    /// and before its answer the key is already here, while in
    /// [`State::outlined`] it is not there yet.
    pub outlines: std::collections::HashSet<String>,
    /// Snapshots whose show awaits a catalogue answer.
    ///
    /// A show is not a selection: it puts a layer into "Now viewing" and does
    /// not touch the checkbox in the list (see handlers::overlay). So "did they
    /// change their mind" cannot be asked from the selection: the catalogue
    /// request lives for seconds, there is no way to kill it, and "Remove from
    /// globe" gets pressed in the meantime.
    pub showing: std::collections::HashSet<String>,
    /// The file with which a snapshot was asked to be put on the globe
    /// (`Msg::GlobeFile`), by snapshot key — until the show starts: the product
    /// is restored at the provider by the catalogue request, and the request
    /// waits for it here (see `handlers::overlay::show`).
    pub file_wishes: std::collections::HashMap<String, String>,
    /// Snapshots that could not be shown, and why — by snapshot key.
    ///
    /// Remembered because this failure is about the snapshot itself, not about
    /// the attempt: "the product has no imagery at all" and "the file only reads
    /// whole" are not cured by a second press, and waiting for them takes tens
    /// of seconds. What the status line says lives until the next word and
    /// vanishes with it; returning to the list a minute later, the person sees
    /// the same inviting icon and presses it again.
    ///
    /// It does not cancel the action, it explains it: the icon stays pressable —
    /// the attempt could also fail over the network — but it glows with a
    /// warning and carries the reason as a tooltip. A new attempt clears the
    /// reason: keeping it after a successful show would be lying.
    pub unshowable: std::collections::HashMap<String, String>,

    // -- Reply routing --
    //
    // A table is keyed by the reply topic, not by the view (see
    // veldsdk::Correlator): "whose id is this" must have one answer. Iterating
    // over views does not answer this question — the view could be closed while
    // the reply was in flight, and the resource that arrived in it is still ours,
    // and only here can it be recognized.
    /// Previews: opening a resource (data-library or data-provider). From there
    /// the canvas leads the show, and the truth about it arrives by the
    /// on_view_state broadcast addressed by view name — there are no
    /// correlations there.
    pub previews: Correlator<ViewId>,
    /// Overlay rasters: opening a resource at the provider. The context is the
    /// overlay key and the raster role; the assembly remembers its correlations
    /// to release them from here when the overlay is removed (see
    /// overlay::Assembly).
    pub opens: Correlator<(String, crate::proto::globe::OverlayRole, u32, overlay::Part)>,
    /// data-provider/on_imagery_result — which rasters the product has. The
    /// context is the key of the overlay that waits for them.
    pub imageries: Correlator<String>,
    /// The same topic, but for previews: a snapshot stored as a folder cannot be
    /// opened directly (a GET on the catalogue path answers 404), and the
    /// provider picks the raster inside it. The context is the preview tab that
    /// waits for it.
    ///
    /// A separate table, not shared with the overlays: the answer is the same,
    /// but there are two waiters, and "whose id is this" must have exactly one
    /// answer.
    pub preview_imagery: Correlator<ViewId>,
    /// data-provider/on_list_path_result. The context is who this listing is
    /// for (see [`Listing`]).
    pub listings: Correlator<Listing>,
    /// data-provider/on_search_result.
    pub searches: Correlator<ViewId>,
    /// globe/on_probe — "what is under the pointer". Not a table, but the last
    /// question: the answer to the previous one is no longer needed, the pointer
    /// has moved on since.
    pub probe: veldsdk::Latest,
    /// data-provider/on_locate_result — a product by storage key. The context is
    /// why it was asked (see [`Locate`]): to put on the globe or to outline.
    pub locates: Correlator<Locate>,
    /// fs/on_write of the layout: correlation → region handed to the service.
    /// The reply releases it — until then the region is ours (see
    /// handlers::persist).
    pub layout_writes: Correlator<u64>,
    /// Reading the saved layout. Not a table, but the last question: it is asked
    /// once, at startup, and an answer to the question before the last would
    /// mean the screen is assembled twice.
    pub layout_read: veldsdk::Latest,
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        // The starting tab comes from the config; the default and the behavior
        // on an unknown value is Search (this view always exists).
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

    /// Creates a pane on the named side and immediately gives it a share — this
    /// is how the screen is assembled from a save (see handlers::persist). The
    /// live path to splitting the screen is different: a tab move splits it (see
    /// [`Self::move_aside`]).
    pub fn split_pane(&mut self, pane: PaneId, side: Side, fraction: f32) -> Option<PaneId> {
        let fresh = self.layout.split(pane, side)?;
        if let Some(split) = self.layout.split_of(fresh) {
            self.layout.set_fraction(split, fraction);
            // The same limit as the live boundary, and in the same way: the
            // share comes from a file, and it can be both edited and written by
            // a different window layout. A pane opened at zero shows neither its
            // tabs nor the boundary to pull to give it room back.
            self.divide(split, 0.0);
        }
        Some(fresh)
    }

    // -- Tabs --

    /// Opens a view in the named pane and makes it active there, and the pane
    /// itself the one at hand. Opening is always a new tab: "reuse a similar
    /// one" is the caller's decision, it has [`Self::find`] for that.
    pub fn open_in(&mut self, pane: PaneId, kind: ViewKind) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        let at = self.after_last(pane);
        self.views.insert(at, View { id, kind, pane });
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
        id
    }

    /// The place in the list right after the last tab of this pane: both opened
    /// and moved tabs go there — the end of the strip is where the person looks
    /// after such an action. For a pane without tabs this is the end of the list.
    fn after_last(&self, pane: PaneId) -> usize {
        self.views
            .iter()
            .rposition(|view| view.pane == pane)
            .map_or(self.views.len(), |at| at + 1)
    }

    /// Removes the view from the set and hands it to the caller: closing a
    /// preview stops the show at the canvas, and it may not publish state itself
    /// — the module's outgoing link must be visible in the schema (see
    /// handlers::nav).
    pub fn close(&mut self, id: ViewId) -> Option<View> {
        let at = self.views.iter().position(|view| view.id == id)?;
        let view = self.views.remove(at);
        if self.layout.active(view.pane) == Some(id) {
            self.layout.set_active(view.pane, self.neighbour(view.pane, at));
        }
        self.forget_if_empty(view.pane);
        Some(view)
    }

    /// Removes an emptied pane: its room goes to the neighbor, and the eye goes
    /// there too. The last pane stays empty — there is no one to give its room
    /// to, and "all tabs are closed" has to be shown somewhere.
    ///
    /// Public not just for closing a tab: an empty pane also comes out of a
    /// saved layout where a singleton is named twice (see
    /// `handlers::persist::restore`), and the rule "there are no empty panes" is
    /// a rule precisely to be one.
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

    /// Who takes the place of a closed tab: the next one in the same pane,
    /// otherwise the previous one.
    ///
    /// Computed over the whole list, not the filtered one: its indices are the
    /// tab order, and adjacency means adjacency within one's own pane.
    fn neighbour(&self, pane: PaneId, at: usize) -> Option<ViewId> {
        let right = self.views.iter().skip(at).find(|view| view.pane == pane);
        let left = self.views.iter().take(at).rev().find(|view| view.pane == pane);
        right.or(left).map(|view| view.id)
    }

    /// Makes the view active in its pane, and the pane the one at hand.
    pub fn focus(&mut self, id: ViewId) {
        let Some(view) = self.views.iter().find(|view| view.id == id) else { return };
        let pane = view.pane;
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
    }

    /// The pane whose widgets currently handle events without an addressee.
    pub fn focused(&self) -> PaneId {
        self.focus
    }

    /// The pane tree — what the screen is assembled from.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Merge everything into one pane: the tabs gather into the one at hand —
    /// that is what was being looked at — and their order is preserved, it is a
    /// property of the list.
    pub fn collapse(&mut self) {
        let kept = self.layout.collapse(self.focus);
        for view in &mut self.views {
            view.pane = kept;
        }
        // The one that was at hand stays active. If the one at hand was an empty
        // pane — the first of the remaining ones: there are tabs, and "all tabs
        // are closed" instead of them would be a lie.
        let active = self.layout.active(kept).or_else(|| self.views.first().map(|view| view.id));
        self.layout.set_active(kept, active);
        self.focus = kept;
    }

    /// Move a tab into the named pane — and move the eye there too.
    pub fn move_to(&mut self, id: ViewId, pane: PaneId) {
        let Some(at) = self.views.iter().position(|view| view.id == id) else { return };
        let from = self.views[at].pane;
        if !self.layout.contains(pane) {
            return;
        }
        // There is nothing to move within one's own pane, but there is
        // something to show: this is called both by the "plus" answered with an
        // already open singleton and by dropping a tab onto its own strip. A
        // silent exit here would mean that the request "show me search here"
        // does not change the screen at all.
        if from == pane {
            self.layout.set_active(pane, Some(id));
            self.focus = pane;
            return;
        }

        let mut view = self.views.remove(at);
        // Who stays shown in the former pane is computed before the insert:
        // the insert shifts the indices the neighbor is searched by.
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

    /// Move a tab to where the person sees the named side: into the neighbor
    /// pane, and if there is none there — into one created on that side.
    ///
    /// Splitting the screen is therefore not a separate action but a consequence
    /// of the move: it is split not in itself, but to show something alongside —
    /// and an empty pane that still needs filling would be an extra step right
    /// where the person already said with what.
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

    /// Put a tab beside the named pane, splitting its room — this is how a drop
    /// onto an edge ends (see `handlers::nav::on_tab_drop`).
    ///
    /// Differs from [`Self::move_aside`] in that it splits the room of the named
    /// pane, not of the one the tab lay in: for a drop those two differ, and it
    /// is asked about the first.
    pub fn drop_beside(&mut self, id: ViewId, pane: PaneId, side: Side) {
        // The only tab of its own pane dropped onto its edge moves nothing: the
        // created pane would at once be left alone in place of the former one,
        // and the screen unchanged.
        if self.pane_of(id) == Some(pane) && self.views_in(pane).count() < 2 {
            return;
        }
        let Some(fresh) = self.split_pane(pane, side, 0.5) else { return };
        self.move_to(id, fresh);
    }

    /// Whether such a move changes the screen. The menu asks this in advance: an
    /// item that does nothing promises a choice and answers with silence.
    pub fn can_move(&self, id: ViewId, side: Side) -> bool {
        let Some(from) = self.pane_of(id) else { return false };
        match self.layout.neighbour(from, side) {
            // Moving into a neighbor always makes sense: it is different.
            Some(_) => true,
            // The only tab of a pane travels with the pane itself: it empties
            // and leaves the tree, and the screen stays the same.
            None => self.views_in(from).count() > 1,
        }
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    /// Tabs of one pane, in their list order.
    pub fn views_in(&self, pane: PaneId) -> impl Iterator<Item = &View> {
        self.views.iter().filter(move |view| view.pane == pane)
    }

    /// The active tab of the named pane — what is shown in it.
    pub fn active_in(&self, pane: PaneId) -> Option<ViewId> {
        self.layout.active(pane)
    }

    /// The pane the tab lies in. `None` — it has already been closed.
    pub fn pane_of(&self, id: ViewId) -> Option<PaneId> {
        self.views.iter().find(|view| view.id == id).map(|view| view.pane)
    }

    pub fn get(&self, id: ViewId) -> Option<&ViewKind> {
        self.views.iter().find(|view| view.id == id).map(|view| &view.kind)
    }

    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut ViewKind> {
        self.views.iter_mut().find(|view| view.id == id).map(|view| &mut view.kind)
    }

    /// The first view satisfying the condition — for "open or show the already
    /// open one".
    pub fn find(&self, matches: impl Fn(&ViewKind) -> bool) -> Option<ViewId> {
        self.views.iter().find(|view| matches(&view.kind)).map(|view| view.id)
    }

    // -- Access to the state of the named view --
    //
    // Exactly the named one, not the active one: there can be any number of
    // panes on screen, and "the active view" does not answer "whose click is
    // this". Who exactly — the message itself says (see `Msg::In`).

    /// Window width in layout points.
    pub fn logical_width(&self) -> f32 {
        self.window.0 as f32 / self.scale.max(1.0)
    }

    /// The height allotted to the pane tree: the window without the status line
    /// beneath it. Dragging a horizontal boundary is measured by it — shares are
    /// counted from the tree's room, not from the window.
    fn layout_height(&self) -> f32 {
        let chrome = crate::module::theme::BAR_HEIGHT + crate::module::theme::HAIRLINE;
        (self.window.1 as f32 / self.scale.max(1.0) - chrome).max(0.0)
    }

    /// A division boundary was dragged: a shift in layout points becomes a shift
    /// of the share. It is counted from the room allotted to the division itself
    /// — divisions can nest, and a nested share is not measured by the window.
    ///
    /// The boundaries themselves are not accounted for in this arithmetic: each
    /// of them is thinner than the precision it is dragged with.
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

    /// The width of the pane the view lies in — what the room for the list
    /// columns is measured by. It cannot be counted from the window: the pane is
    /// narrower, the columns do not get thinner because of that, and the
    /// stretching name would collapse to zero (see `table::fit`).
    ///
    /// The share comes from the tree: there can be several divisions, and nested
    /// shares multiply (see `Layout::share`).
    pub fn pane_width(&self, id: ViewId) -> f32 {
        let share = self.pane_of(id).map_or(1.0, |pane| self.layout.share(pane).0);
        self.logical_width() * share
    }

    /// List display settings: messages edit them, the same for all list views.
    pub fn listing_mut(&mut self, id: ViewId) -> Option<&mut listing::ListingState> {
        self.get_mut(id)?.listing_mut()
    }

    /// Close whatever is expanded. Called from everywhere an action leads away
    /// from the place the menu stands over.
    pub fn close_menus(&mut self) {
        self.open = Open::Nothing;
    }

    /// Expand the named thing — or close it if already expanded: pressing a chip
    /// again closes its menu.
    pub fn toggle_open(&mut self, what: Open) {
        self.open = match self.open == what {
            true => Open::Nothing,
            false => what,
        };
    }

    /// The expanded list menu of the named tab. `None` means something else is
    /// expanded: another tab's menu, a layer menu, the tab strip, or nothing.
    pub fn menu_in(&self, view: ViewId) -> Option<&listing::Menu> {
        match &self.open {
            Open::Listing(id, menu) if *id == view => Some(menu),
            _ => None,
        }
    }

    /// Whether the menu of the named "Now viewing" layer is expanded.
    pub fn layer_menu(&self, key: &str) -> bool {
        matches!(&self.open, Open::Layer(open) if open == key)
    }

    /// Whether the variable list under the frame of the named preview tab is
    /// expanded.
    pub fn variables_menu(&self, view: ViewId) -> bool {
        matches!(&self.open, Open::Variables(open) if *open == view)
    }

    /// Whether the variable list in the row of the named "Now viewing" layer is
    /// expanded.
    pub fn layer_variables(&self, key: &str) -> bool {
        matches!(&self.open, Open::LayerVariables(open) if open == key)
    }

    /// A change of the list filter: the page resets, the expanded thing closes.
    /// A common tail for all filter edits — what is typed changes the list's
    /// composition, and the previous page belonged to a different list.
    pub fn refine(&mut self, view: ViewId) {
        self.close_menus();
        if let Some(listing) = self.listing_mut(view) {
            listing.page = 0;
        }
    }


    /// Key of the snapshot ringed on the globe by the ribbon; empty — this is
    /// not what is highlighted.
    pub fn picked_key(&self) -> &str {
        self.highlight.as_ref().filter(|it| it.on_globe).map_or("", |it| it.key.as_str())
    }

    /// The row a jump led to in the named tab; empty — the jump was not here or
    /// there was none.
    pub fn target_in(&self, view: ViewId) -> &str {
        self.highlight
            .as_ref()
            .filter(|it| it.view == Some(view))
            .map_or("", |it| it.key.as_str())
    }

    /// Put out the ribbon on the globe without touching the jump highlight: the
    /// jump did not set it. `true` — there was something to put out.
    pub fn deselect(&mut self) -> bool {
        let Some(highlight) = self.highlight.as_mut().filter(|it| it.on_globe) else {
            return false;
        };
        highlight.on_globe = false;
        // A highlight without a ribbon and without a tab lights nothing.
        if highlight.view.is_none() {
            self.highlight = None;
        }
        true
    }

    /// Put out the jump highlight in the named tab: we left its page, and on
    /// another it would speak untruth. The ribbon on the globe stays.
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

    /// The globe of the named view. `None` — the tab was closed or swapped while
    /// the event was in flight.
    pub fn globe_mut(&mut self, id: ViewId) -> Option<&mut globe::GlobeState> {
        match self.get_mut(id)? {
            ViewKind::Globe(globe) => Some(globe),
            _ => None,
        }
    }

    /// The preview of the named view. `None` — the tab was closed while the
    /// answer was in flight; the resource that arrived is still ours, the caller
    /// releases it.
    pub fn preview_mut(&mut self, id: ViewId) -> Option<&mut preview::PreviewState> {
        match self.get_mut(id)? {
            ViewKind::Preview(preview) => Some(preview),
            _ => None,
        }
    }

    /// The snapshot selected by a click on the globe, together with its
    /// geometry. `None` — nothing is selected: the click missed the outlines or
    /// the outline was removed, and there is no outline anymore.
    pub fn picked(&self) -> Option<&globe::Outlined> {
        let key = self.picked_key();
        self.outlined.iter().find(|outlined| outlined.key == key)
    }

}

/// Whether the selection takes a batch action — by the answer to the action.
///
/// The handler counts it itself (`handlers::library::batch`), by the same
/// iteration it later acts with: these numbers decide whether to show the
/// button, and a second answer to "what will be done" would one day diverge
/// from the first — the button would promise an action and not perform it.
pub struct Batch {
    /// Whether there is anything to delete — at least one library record.
    pub deletable: bool,
    /// Whether there is anything to put into download.
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
