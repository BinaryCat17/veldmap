//! Markup messages: what a widget tells the module.
//!
//! One type for every press and all input. The view sends it (`on_press`,
//! `on_input`), `module::on_ui_event` receives it with an exhaustive match — so
//! a variant cannot be added and forgotten, nor can a nonexistent one be handled.
//!
//! The bus carries the method name, the payload, and the addressee (see
//! `UiMessage`). The names are the module's private wiring: ui-service returns
//! them as an echo, neither knowing nor checking their meaning.
//!
//! **The addressee is separate from the payload.** Any number of panels can be
//! on screen, and the "active view" does not answer "whose click is this": the
//! click could have happened in a panel that is not at hand. So everything born
//! in a tab body travels wrapped in [`Msg::In`], and the tab names itself via the
//! `Handler.key` field — exactly the one the renderer does not substitute. It
//! could not go into the payload: for a text input, an area, and a slider the
//! payload is made by the renderer.

use crate::module::state::listing::{choice, Choice, Filter, Grouping, Menu, Sorting};
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{PaneId, Shift, Side, SplitId, ViewId};
use crate::proto::ui_service::{DropEvent, PointerEvent, UiEventResponse, ViewportSize};
use veld_ui_service_wrap::{Payload, UiMessage};

choice! {
    /// What can be opened as a new tab. One variant instead of five messages:
    /// they differ only in which view to create, while the panel to put it in
    /// is the same concern for all of them.
    ///
    /// The order is the order of the "plus" menu items. The empty tab goes last:
    /// it is not about what to look at, but about the place where that is decided
    /// later.
    NewTab {
        Browse     = "browse",     "Network catalog";
        Search     = "search",     "Scene search";
        Downloaded = "downloaded", "Downloaded";
        Globe      = "globe",      "Globe";
        Shown      = "shown",      "On display";
        /// A tab that shows nothing yet (see `ViewKind::Empty`).
        Empty      = "empty",      "Empty tab";
    }
}

impl NewTab {
    /// What fills an empty tab — the same five without itself: an empty tab is
    /// not filled with an empty one.
    pub const KINDS: [NewTab; 5] =
        [NewTab::Browse, NewTab::Search, NewTab::Downloaded, NewTab::Globe, NewTab::Shown];
}

/// Everything the table knows about a message set: how each message is declared
/// in markup, whom it addresses, and how it is read back.
///
/// A trait, not a trio of free functions: `Msg` asks the nested set for exactly
/// the same thing it answers with itself (see [`Msg::In`]).
trait Wired: Sized {
    /// Method name and declared payload.
    fn declared(&self) -> (&'static str, String);
    /// Whom the message addresses; empty means nobody, and that is legal.
    fn named(&self) -> String;
    /// `None` means the method is not ours: this message does not belong to the set.
    fn parse(event: &UiEventResponse) -> Option<Self>;
}

/// Declares a message set together with how each message travels over the bus.
///
/// One place for four lists: the variant itself, the method name, payload
/// assembly, and payload parsing. Written separately, they are held together by
/// attention alone, and unevenly at that: assembly matches the enumeration
/// exhaustively and cannot be forgotten, while parsing matches the method name
/// as a string and ends in "anything else — not ours", so a forgotten parsing arm
/// compiles silently and turns a press into an "unrecognized markup message" in
/// the log. Both are generated from the table, and there is nothing left to
/// forget: one message — one row.
///
/// A row names the variant, its fields, and the method name. Each field is
/// prefixed with its kind — how it rides in `UiEventResponse`:
///
/// * `val` — the payload, named by the markup itself;
/// * `sub` — the payload supplied by the renderer: it cannot be declared, but
///   it is read like any other;
/// * `key` — the addressee.
///
/// How each type's payload travels is stated once per type — in [`Value`] and
/// [`Addressee`] — rather than once per message.
///
/// After the table a variant is declared that has no method name of its own:
/// the nested set names it, and it is recognized by that name during parsing. A
/// table row cannot express this, so it stands separately — and therefore last
/// in the enumeration.
macro_rules! messages {
    (
        $(#[$outer:meta])*
        $name:ident {
            $(
                $(#[$doc:meta])*
                $variant:ident $(( $($slot:ident $ty:ty),+ ))? = $method:literal;
            )*
        }
        $(
            $(#[$tail_doc:meta])*
            $tail:ident(key $tail_key:ty, nested $tail_msg:ty);
        )?
    ) => {
        $(#[$outer])*
        pub enum $name {
            $( $(#[$doc])* $variant $(( $($ty),+ ))?, )*
            $( $(#[$tail_doc])* $tail($tail_key, $tail_msg), )?
        }

        impl Wired for $name {
            fn declared(&self) -> (&'static str, String) {
                match self {
                    $(
                        $name::$variant $(( $( messages!(@declaring $slot $slot) ),+ ))? =>
                            ($method, messages!(@declare $($( $slot $slot )+)?)),
                    )*
                    $( $name::$tail(_, inner) => inner.declared(), )?
                }
            }

            fn named(&self) -> String {
                match self {
                    $(
                        $name::$variant $(( $( messages!(@naming $slot $slot) ),+ ))? =>
                            messages!(@name $($( $slot $slot )+)?),
                    )*
                    $( $name::$tail(view, _) => view.name(), )?
                }
            }

            fn parse(event: &UiEventResponse) -> Option<Self> {
                Some(match event.method.as_str() {
                    $(
                        $method =>
                            $name::$variant $(( $( messages!(@read $slot $ty, event) ),+ ))?,
                    )*
                    _ => return messages!(@rest event $(, $name $tail $tail_key, $tail_msg)?),
                })
            }
        }
    };

    // A field names itself only in the assembly that asks for it: a bound but
    // unused field would be a compiler warning on every table row.
    (@declaring val $bound:ident) => { $bound };
    (@declaring sub $bound:ident) => { _ };
    (@declaring key $bound:ident) => { _ };
    (@naming key $bound:ident) => { $bound };
    (@naming val $bound:ident) => { _ };
    (@naming sub $bound:ident) => { _ };

    // No more than one field carries the payload, and likewise for the
    // addressee: the other fields are skipped, and if none is found — empty.
    (@declare) => { String::new() };
    (@declare val $bound:ident $($rest:tt)*) => { $bound.declare() };
    (@declare sub $bound:ident $($rest:tt)*) => { messages!(@declare $($rest)*) };
    (@declare key $bound:ident $($rest:tt)*) => { messages!(@declare $($rest)*) };
    (@name) => { String::new() };
    (@name key $bound:ident $($rest:tt)*) => { $bound.name() };
    (@name val $bound:ident $($rest:tt)*) => { messages!(@name $($rest)*) };
    (@name sub $bound:ident $($rest:tt)*) => { messages!(@name $($rest)*) };

    (@read val $ty:ty, $event:ident) => { <$ty as Value>::read($event)? };
    (@read sub $ty:ty, $event:ident) => { <$ty as Value>::read($event)? };
    (@read key $ty:ty, $event:ident) => { <$ty as Addressee>::read($event)? };

    // Name not from the table: either the nested set knows it, or it is not ours.
    (@rest $event:ident) => { None };
    (@rest $event:ident, $name:ident $tail:ident $tail_key:ty, $tail_msg:ty) => {
        Some($name::$tail(
            <$tail_key as Addressee>::read($event)?,
            <$tail_msg as Wired>::parse($event)?,
        ))
    };

    // Numbers ride as strings and are parsed back the same way: an unfit string
    // is not a number, and so not a message.
    (@numbers $($ty:ty),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                self.to_string()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                event.value().parse().ok()
            }
        }
    )+ };

    // Payload made by the renderer: it cannot be declared in markup — only the
    // renderer knows where each zone is and where the pointer landed.
    (@substituted $($ty:ty => $carried:ident),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                String::new()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                event.$carried().cloned()
            }
        }
    )+ };

    // `Choice` sets ride by their own key — all alike.
    (@choices $($ty:ty),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                self.key().to_string()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                <$ty>::from_key(event.value())
            }
        }
    )+ };

    // Identifiers address the same way — both on their own and optional.
    (@ids $($ty:ty),+ $(,)?) => { $(
        impl Addressee for $ty {
            fn name(&self) -> String {
                self.to_string()
            }

            /// Identifier parsing happens here and only here: it comes back as
            /// a string, and the place where the string becomes an identifier
            /// again must be exactly one.
            fn read(event: &UiEventResponse) -> Option<Self> {
                event.key.parse().ok()
            }
        }

        impl Addressee for Option<$ty> {
            fn name(&self) -> String {
                self.map(|id| id.to_string()).unwrap_or_default()
            }

            /// An unparsed name means "nobody": an empty key carries both
            /// "close" and a message that needs no addressee.
            fn read(event: &UiEventResponse) -> Option<Self> {
                Some(event.key.parse().ok())
            }
        }
    )+ };
}

messages! {
    Msg {
        // -- Tabs --
        //
        // A tab is addressed by identifier, not by position: the position
        // changes when a neighbor is closed.
        TabSelect(key ViewId) = "tab_select";
        TabClose(key ViewId) = "tab_close";
        /// The "plus" menu of the named panel; `None` closes what is open.
        TabMenu(key Option<PaneId>) = "tab_menu";
        /// The tab's own menu: move, close. `None` closes.
        TabOptions(key Option<ViewId>) = "tab_options";
        /// Create a tab in the named panel.
        NewTab(key PaneId, val NewTab) = "new_tab";
        /// Move the tab to where the user sees this side: into the neighboring
        /// panel or into one created on this side (see `State::move_aside`).
        TabMove(key ViewId, val Side) = "tab_move";
        /// Merge all panels into one.
        TabCollapse = "tab_collapse";
        /// The split divider was dragged: the offset in markup points. What that
        /// means in fractions is known only to whoever set them (see
        /// `State::divide`).
        Divide(key SplitId, sub f32) = "divide";
        /// The divider was released. A separate message, not the end of the
        /// offset stream: "the drag is over" is a moment, not an absence of
        /// events, and waiting for it in silence would mean never reaching it.
        Divided = "divided";
        /// A tab was dropped onto a panel. What was carried and which edge of the
        /// panel it hit is made by the renderer: only it knows where each zone is
        /// (see `Drop` in ui-service/types.proto). The payload travels as-is —
        /// the handler parses it, exactly like for the pointer over an area.
        TabDrop(key PaneId, sub DropEvent) = "tab_drop";

        // -- Records --
        //
        // They act on the library, not on a view, so they carry no source tab:
        // downloads are one per window, no matter which panel asks.
        //
        // A download's snapshot rides in the key: the payload is taken by the
        // file itself, and "what it belongs to" is the same "with whom" question
        // the key answers.
        /// Download, resume, or pause — by provider key. The second field is the
        /// snapshot the file belongs to (empty — on its own): the library writes
        /// it into its own snapshot and cannot derive that from the key.
        Download(val String, key String) = "download";
        /// Discard a record: delete the downloaded or abandon the started. One
        /// message for both, because they leave behind the same thing — nothing;
        /// only the menu label makes them different.
        Delete(val String) = "delete";
        /// Discard a whole snapshot — all its files. The key here is the snapshot,
        /// not the record: "snapshot" is a display notion (the library keeps track
        /// of files), and the one who assembled it expands it into records (see
        /// `handlers::library::on_delete_snapshot`).
        DeleteSnapshot(val String) = "delete_snapshot";
        /// Download a whole snapshot — by product key. The file list comes from a
        /// recursive listing by the provider: only it knows the .SAFE layout, and
        /// the download goes file by file (see
        /// `handlers::library::on_download_snapshot`).
        DownloadSnapshot(val String) = "download_snapshot";
        /// Pause the download of a whole snapshot. The key is the snapshot — for
        /// the same reason as discarding: the one who assembled it expands it
        /// into files.
        PauseSnapshot(val String) = "pause_snapshot";
        /// Show the record in the file manager. The library computes the path —
        /// the storage layout is its own — while the platform does the showing.
        Reveal(val String) = "reveal";

        // -- Globe --
        /// Remove everything from the globe: overlays and outlines alike. There
        /// is no way to remove them separately, and no reason to separate them
        /// for the user — one globe is what the user sees.
        GlobeClear = "globe_clear";

        // -- On display --
        //
        // A layer is addressed by the product key, and it rides in the same
        // field as the tab in `In`: their question is the same — "with whom" —
        // and answering it in two ways would be needless.
        /// Layer opacity, 0..1.
        OverlayOpacity(key String, sub f32) = "overlay_opacity";
        /// Hide the layer or put it back on the globe.
        OverlayHidden(key String, val bool) = "overlay_hidden";
        /// Remove the layer entirely: resources are released; bringing it back
        /// means reopening the snapshot.
        OverlayRemove(key String) = "overlay_remove";
        /// Move the layer within the set: the set's order is the order of layers
        /// on the globe from bottom to top, and there is no other way to say
        /// "this one above that one".
        OverlayShift(key String, val Shift) = "overlay_shift";
        /// Hide or show all layers at once.
        OverlayHideAll(val bool) = "overlay_hide_all";
        /// Layer menu: ordering and jumps to the snapshot. `None` closes what is
        /// open.
        OverlayMenu(key Option<String>) = "overlay_menu";
        /// Expand the variable list of the layer's detailed file or close the
        /// expanded one (`None`).
        OverlayVariables(key Option<String>) = "overlay_variables";
        /// Show another variable of the layer's file on the globe — a path from
        /// the list.
        OverlayVariable(key String, val String) = "overlay_variable";
        /// Put a snapshot on the globe with this file: the snapshot key and the
        /// key of the file within it that becomes the detailed raster.
        GlobeFile(key String, val String) = "globe_file";

        // -- Outlines --
        //
        // An outline is addressed by the same snapshot key as a layer, and in
        // the same field: on the globe they refer to the same snapshot, just at
        // different detail.
        /// Outline the snapshot on the globe or remove the outline — the icon in
        /// the list row. This touches neither the checkbox selection nor the
        /// raster display: the three row states are independent (see
        /// handlers::outline).
        OutlineToggle(key String) = "outline_toggle";
        /// Remove the outline — the "On display" list row, where it stands as its
        /// own row and there is nowhere else to remove it from.
        OutlineRemove(key String) = "outline_remove";
        /// Aim the globe at the outline and select it.
        OutlineFocus(key String) = "outline_focus";
    }
    // This variant has no method name of its own — the nested message names it,
    // and it is recognized by that name during parsing; a table row cannot
    // express this.
    /// A message born in the body of the named tab.
    In(key ViewId, nested ViewMsg);
}

messages! {
    /// What a widget says inside a tab. A separate type, not two dozen variants
    /// with `ViewId` as the first field: their addressee is shared and is
    /// assigned once — in [`Msg::In`].
    ViewMsg {
        /// What to fill an empty tab with. The tab, not the panel: the tab
        /// becomes the chosen view in place, without creating a second one beside
        /// it or leaving itself empty (see `handlers::nav::fill`).
        Fill(val NewTab) = "fill";

        // -- List display --
        /// Expand a menu or close the expanded one (`None`).
        OpenMenu(val Option<Menu>) = "open_menu";
        Filter(val Filter) = "filter";
        Group(val Grouping) = "group";
        Sort(val Sorting) = "sort";
        /// What was typed in the filter field — supplied by the renderer, not by
        /// the markup.
        Query(val String) = "query";
        Page(val usize) = "page";
        /// Expand a row into its contents or collapse it back.
        Expand(val String) = "expand";
        /// Select a row or deselect it — the set for batch actions. The globe is
        /// unaffected: outline and display set their own icons.
        Check(val String) = "check";
        /// Select or deselect everything shown at once — the checkbox in the
        /// header. It acts on exactly what it says: the selectable rows of this
        /// page.
        CheckShown(val bool) = "check_shown";
        /// Clear the entire list selection, including what was made in another
        /// folder — the button in the header. A separate message, not
        /// `CheckShown(false)`: the selection survives folder navigation, and
        /// "deselect visible" would leave selected what is no longer on screen.
        CheckClear = "check_clear";
        /// Download everything selected in this list.
        CheckDownload = "check_download";
        /// Delete everything selected in this list — from disk and from the
        /// download queue.
        CheckDelete = "check_delete";

        // -- Catalog search --
        //
        // Filtering the list (`Query`) and querying the catalog are different
        // things and must not be confused: the first narrows what is already
        // found, the second goes over the network and changes what was found at
        // all.
        /// What was typed in the query field.
        SearchQuery(val String) = "search_query";
        /// How to narrow the query. Each of the three resubmits it: the choice is
        /// made with one press, and there is nothing to ask confirmation for
        /// afterwards.
        SearchMission(val Mission) = "search_mission";
        SearchPeriod(val Period) = "search_period";
        /// The bounds of its own interval — typed into the date fields (see
        /// `Period::Custom`).
        SearchFrom(val String) = "search_from";
        SearchTo(val String) = "search_to";
        SearchCloud(val Cloud) = "search_cloud";
        /// Send the query to the catalog.
        RunSearch = "run_search";

        // -- Catalog --
        /// Go to a folder by provider key; an empty key rereads the current one.
        Enter(val String) = "enter";
        Up = "up";
        /// Show the named record in the catalog: open the folder it lies in, go
        /// to its page, and highlight it.
        ///
        /// Separate from [`ViewMsg::Enter`] because the question is different:
        /// that one names a folder and leads to it, this one names a record and
        /// leads to it. The catalog is reused meanwhile — only whoever was asked
        /// to create a tab creates one (see `handlers::nav::catalog`).
        InCatalog(val String) = "in_catalog";

        // -- Snapshot preview --
        //
        // They open a new tab — in the same half where the row stands: viewing
        // next to the list is exactly why the screen is split.
        /// View what is downloaded — by the library record name.
        Preview(val String) = "preview";
        /// View what is not yet downloaded — by provider key.
        PreviewRemote(val String) = "preview_remote";
        /// Show a snapshot stored as a folder: the provider picks the raster
        /// inside (see `handlers::preview::on_view_product_pressed`).
        PreviewProduct(val String) = "preview_product";

        // -- Preview canvas --
        //
        // The renderer supplies the area's payload (as on the globe); toolbar
        // buttons ride as their own messages.
        /// The area under the frame got a new size — in pixels of its texture.
        PreviewResized(sub ViewportSize) = "preview_resized";
        /// Pointer over the canvas, in the same pixels.
        PreviewPointer(sub PointerEvent) = "preview_pointer";
        /// Fit the snapshot into the canvas.
        PreviewFit = "preview_fit";
        /// A zoom step around the center: the sign is the direction.
        PreviewZoom(val f32) = "preview_zoom";
        /// Expand the variable list of the file under the frame or close it.
        PreviewVariables(val bool) = "preview_variables";
        /// Show another variable of the file — a path from the list under the frame.
        PreviewVariable(val String) = "preview_variable";

        // -- Globe --
        //
        // The renderer supplies the payload for both, so in markup they are
        // declared by constructors rather than ready values (see `viewport`).
        /// The area under the globe got a new size — in pixels of its texture.
        GlobeResized(sub ViewportSize) = "globe_resized";
        /// Pointer over the area, in the same pixels.
        GlobePointer(sub PointerEvent) = "globe_pointer";
        /// Put a snapshot on the globe as a raster — or take it off. By provider
        /// key; its own payload — it comes not from the area but from a list row.
        ///
        /// As a toggle, and with the same name as the outline (`outline_toggle`):
        /// their question is the same — whether the snapshot lies on the globe —
        /// and with different mechanics they would read as two different kinds of
        /// action. The camera is unaffected: `outline_focus` aims it.
        GlobeToggle(val String) = "globe_toggle";
    }
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = self.declared();
        (method.to_string(), value)
    }

    /// Whom the message addresses: a tab, a panel, or a layer on the globe.
    ///
    /// There are as many identical widgets on screen as tabs and snapshots, and
    /// they cannot be told apart by method name — while the payload of half of
    /// them is taken by the renderer (see `Handler.key` in ui-service/types.proto).
    fn key(&self) -> String {
        self.named()
    }

    fn decode(event: &UiEventResponse) -> Option<Self> {
        Msg::parse(event)
    }
}

// -- How payload and addressee travel --
//
// Once per type, not per message: a string rides as a string identically across
// the two dozen messages that carry it, and the rule belongs to the type.

/// Payload: the `Handler.value` field going out, `UiEventResponse.payload`
/// coming back.
///
/// `read` answers `None` when the payload is unfit, and parsing of the whole
/// message stops there. The payload kind need not be checked: for an event of
/// another kind the string is empty, and parsing an empty string as a number or
/// a set key will itself say there is nothing to read.
trait Value: Sized {
    /// What to declare in markup.
    fn declare(&self) -> String;
    fn read(event: &UiEventResponse) -> Option<Self>;
}

/// Addressee: the `Handler.key` field. Separate from the payload — see the file
/// header.
trait Addressee: Sized {
    fn name(&self) -> String;
    fn read(event: &UiEventResponse) -> Option<Self>;
}

impl Value for String {
    fn declare(&self) -> String {
        self.clone()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.value().to_string())
    }
}

impl Value for bool {
    fn declare(&self) -> String {
        self.to_string()
    }

    /// Everything that is not "true" is "false": a checkbox has no third state,
    /// and an unknown word means the same as an unticked box.
    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.value() == "true")
    }
}

messages!(@numbers usize, f32);
messages!(@choices NewTab, Filter, Grouping, Sorting, Mission, Period, Cloud, Side, Shift);
messages!(@substituted ViewportSize => size, PointerEvent => pointer, DropEvent => drop);

/// Menu is a set where "nothing expanded" is a legal value: an empty name
/// carries both "close" and an unknown name, and there is nothing to show for
/// it (see [`Menu::from_key`]). So reading here never fails.
impl Value for Option<Menu> {
    fn declare(&self) -> String {
        self.as_ref().map(Menu::key).unwrap_or_default()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(Menu::from_key(event.value()))
    }
}

impl Addressee for String {
    fn name(&self) -> String {
        self.clone()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.key.clone())
    }
}

impl Addressee for Option<String> {
    fn name(&self) -> String {
        self.clone().unwrap_or_default()
    }

    /// An empty name means "nobody": a layer's name is never empty, so an empty
    /// key carries "close what is open".
    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(Some(event.key.clone()).filter(|key| !key.is_empty()))
    }
}

messages!(@ids ViewId, PaneId, SplitId);
