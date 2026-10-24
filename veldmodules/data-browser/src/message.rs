use crate::module::state::listing::{choice, Choice, Filter, Grouping, Menu, Sorting};
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{PaneId, Shift, Side, SplitId, ViewId};
use crate::proto::ui_service::{DropEvent, PointerEvent, UiEventResponse, ViewportSize};
use veld_ui_service_wrap::{Payload, UiMessage};

choice! {
    NewTab {
        Browse     = "browse",     "Network catalog";
        Search     = "search",     "Scene search";
        Downloaded = "downloaded", "Downloaded";
        Globe      = "globe",      "Globe";
        Shown      = "shown",      "On display";
        Empty      = "empty",      "Empty tab";
    }
}

impl NewTab {
    pub const KINDS: [NewTab; 5] =
        [NewTab::Browse, NewTab::Search, NewTab::Downloaded, NewTab::Globe, NewTab::Shown];
}

trait Wired: Sized {
    fn declared(&self) -> (&'static str, String);
    fn named(&self) -> String;
    fn parse(event: &UiEventResponse) -> Option<Self>;
}

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

    (@declaring val $bound:ident) => { $bound };
    (@declaring sub $bound:ident) => { _ };
    (@declaring key $bound:ident) => { _ };
    (@naming key $bound:ident) => { $bound };
    (@naming val $bound:ident) => { _ };
    (@naming sub $bound:ident) => { _ };

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

    (@rest $event:ident) => { None };
    (@rest $event:ident, $name:ident $tail:ident $tail_key:ty, $tail_msg:ty) => {
        Some($name::$tail(
            <$tail_key as Addressee>::read($event)?,
            <$tail_msg as Wired>::parse($event)?,
        ))
    };

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

    (@ids $($ty:ty),+ $(,)?) => { $(
        impl Addressee for $ty {
            fn name(&self) -> String {
                self.to_string()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                event.key.parse().ok()
            }
        }

        impl Addressee for Option<$ty> {
            fn name(&self) -> String {
                self.map(|id| id.to_string()).unwrap_or_default()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                Some(event.key.parse().ok())
            }
        }
    )+ };
}

messages! {
    Msg {
        TabSelect(key ViewId) = "tab_select";
        TabClose(key ViewId) = "tab_close";
        TabMenu(key Option<PaneId>) = "tab_menu";
        TabOptions(key Option<ViewId>) = "tab_options";
        NewTab(key PaneId, val NewTab) = "new_tab";
        TabMove(key ViewId, val Side) = "tab_move";
        TabCollapse = "tab_collapse";
        Divide(key SplitId, sub f32) = "divide";
        Divided = "divided";
        TabDrop(key PaneId, sub DropEvent) = "tab_drop";

        Download(val String, key String) = "download";
        Delete(val String) = "delete";
        DeleteSnapshot(val String) = "delete_snapshot";
        DownloadSnapshot(val String) = "download_snapshot";
        PauseSnapshot(val String) = "pause_snapshot";
        Reveal(val String) = "reveal";

        GlobeClear = "globe_clear";

        OverlayOpacity(key String, sub f32) = "overlay_opacity";
        OverlayHidden(key String, val bool) = "overlay_hidden";
        OverlayRemove(key String) = "overlay_remove";
        OverlayShift(key String, val Shift) = "overlay_shift";
        OverlayHideAll(val bool) = "overlay_hide_all";
        OverlayMenu(key Option<String>) = "overlay_menu";
        OverlayVariables(key Option<String>) = "overlay_variables";
        OverlayVariable(key String, val String) = "overlay_variable";
        GlobeFile(key String, val String) = "globe_file";

        OutlineToggle(key String) = "outline_toggle";
        OutlineRemove(key String) = "outline_remove";
        OutlineFocus(key String) = "outline_focus";
    }
    In(key ViewId, nested ViewMsg);
}

messages! {
    ViewMsg {
        Fill(val NewTab) = "fill";

        OpenMenu(val Option<Menu>) = "open_menu";
        Filter(val Filter) = "filter";
        Group(val Grouping) = "group";
        Sort(val Sorting) = "sort";
        Query(val String) = "query";
        Page(val usize) = "page";
        Expand(val String) = "expand";
        Check(val String) = "check";
        CheckShown(val bool) = "check_shown";
        CheckClear = "check_clear";
        CheckDownload = "check_download";
        CheckDelete = "check_delete";

        SearchQuery(val String) = "search_query";
        SearchMission(val Mission) = "search_mission";
        SearchPeriod(val Period) = "search_period";
        SearchFrom(val String) = "search_from";
        SearchTo(val String) = "search_to";
        SearchCloud(val Cloud) = "search_cloud";
        RunSearch = "run_search";

        Enter(val String) = "enter";
        Up = "up";
        InCatalog(val String) = "in_catalog";

        Preview(val String) = "preview";
        PreviewRemote(val String) = "preview_remote";
        PreviewProduct(val String) = "preview_product";

        PreviewResized(sub ViewportSize) = "preview_resized";
        PreviewPointer(sub PointerEvent) = "preview_pointer";
        PreviewFit = "preview_fit";
        PreviewZoom(val f32) = "preview_zoom";
        PreviewVariables(val bool) = "preview_variables";
        PreviewVariable(val String) = "preview_variable";

        GlobeResized(sub ViewportSize) = "globe_resized";
        GlobePointer(sub PointerEvent) = "globe_pointer";
        GlobeToggle(val String) = "globe_toggle";
    }
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = self.declared();
        (method.to_string(), value)
    }

    fn key(&self) -> String {
        self.named()
    }

    fn decode(event: &UiEventResponse) -> Option<Self> {
        Msg::parse(event)
    }
}

trait Value: Sized {
    fn declare(&self) -> String;
    fn read(event: &UiEventResponse) -> Option<Self>;
}

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

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.value() == "true")
    }
}

messages!(@numbers usize, f32);
messages!(@choices NewTab, Filter, Grouping, Sorting, Mission, Period, Cloud, Side, Shift);
messages!(@substituted ViewportSize => size, PointerEvent => pointer, DropEvent => drop);

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

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(Some(event.key.clone()).filter(|key| !key.is_empty()))
    }
}

messages!(@ids ViewId, PaneId, SplitId);
