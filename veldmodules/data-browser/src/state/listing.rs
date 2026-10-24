//! state/listing.rs — what makes a list differ from itself: filtering,
//! grouping, ordering, the page, and the open menu.
//!
//! One shape for all three views: the network catalogue, search, and
//! downloaded are the same list over different sources, and their display
//! settings are shared by design, not by coincidence. Each view keeps its own
//! copy because the setting belongs to the tab: the neighbouring one has a
//! different folder open with its own filter.

use crate::module::components::RowStatus;

/// A dropdown value: a finite set that travels to the markup and back as a
/// string. One trait for all three lists — otherwise each has its own
/// `all/label/parse` and they drift apart.
pub trait Choice: Copy + PartialEq + Sized + 'static {
    /// All values in order — the menu is laid out by them.
    const ALL: &'static [Self];
    /// Key in the markup message: a name, not an index, so the menu order can
    /// change freely.
    fn key(self) -> &'static str;
    /// Label in the menu — full.
    fn title(self) -> &'static str;
    /// Label on the chip itself. Defaults to the menu one: not every set has a
    /// shorter form, and it differs only where nearby text already says what it
    /// refers to.
    fn label(self) -> &'static str {
        self.title()
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|choice| choice.key() == key)
    }
}

/// Declares a set together with its wire key and labels.
///
/// One place for all four lists: the variants themselves, the menu order, the
/// key a variant travels to the markup and back with, and how it is named for a
/// person. Written as four separate `match`es, they drift silently — a missing
/// `key` branch gives not a compile error but a click that fails to resolve.
///
/// The first variant is the set's default. The chip label is optional: if not
/// written, it matches the menu.
macro_rules! choice {
    (
        $(#[$outer:meta])*
        $name:ident {
            $(#[$head_doc:meta])* $head:ident = $head_key:literal, $head_title:literal
                $(, $head_label:literal)? ;
            $( $(#[$doc:meta])* $variant:ident = $key:literal, $title:literal
                $(, $label:literal)? ; )*
        }
    ) => {
        $(#[$outer])*
        #[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
        pub enum $name {
            #[default]
            $(#[$head_doc])*
            $head,
            $( $(#[$doc])* $variant, )*
        }

        impl $crate::module::state::listing::Choice for $name {
            const ALL: &'static [Self] = &[$name::$head, $( $name::$variant ),*];

            fn key(self) -> &'static str {
                match self {
                    $name::$head => $head_key,
                    $( $name::$variant => $key, )*
                }
            }

            fn title(self) -> &'static str {
                match self {
                    $name::$head => $head_title,
                    $( $name::$variant => $title, )*
                }
            }

            fn label(self) -> &'static str {
                match self {
                    $name::$head => choice!(@label $head_title $(, $head_label)?),
                    $( $name::$variant => choice!(@label $title $(, $label)?), )*
                }
            }
        }
    };
    (@label $title:literal) => { $title };
    (@label $title:literal, $label:literal) => { $label };
}

pub(crate) use choice;

choice! {
    /// Filtering by record state.
    Filter {
        All         = "all",         "All",          "all";
        Downloading = "downloading", "Downloading",  "downloading";
        Interrupted = "interrupted", "Interrupted",  "interrupted";
        OnDisk      = "on-disk",     "On disk",      "on disk";
        Remote      = "remote",      "In storage",   "in storage";
    }
}

impl Filter {
    pub fn matches(self, status: &RowStatus) -> bool {
        match self {
            Filter::All => true,
            Filter::Downloading => matches!(status, RowStatus::Downloading { .. }),
            Filter::Interrupted => matches!(status, RowStatus::Paused { .. }),
            Filter::OnDisk => matches!(status, RowStatus::Complete | RowStatus::Partial { .. }),
            Filter::Remote => matches!(status, RowStatus::Remote),
        }
    }
}

choice! {
    /// How to group the rows of one folder.
    Grouping {
        None = "none", "No grouping", "none";
        /// A header for the folder as a whole — one level.
        Folder = "folder", "By folder", "folder";
        /// A header for each path segment — as many segments, as many levels.
        Tree = "tree", "Full tree", "tree";
    }
}

choice! {
    /// Row order.
    Sorting {
        Newest = "newest", "Newest first", "newest";
        Name   = "name",   "By name",      "name";
        Size   = "size",   "By size",      "size";
    }
}

/// Which of the list's menus it is. Whether anything is open at all is told not
/// by this type but by `State::open`: "nothing open" is a property of the
/// screen, not of the list.
#[derive(Clone, PartialEq, Eq)]
pub enum Menu {
    Filter,
    Grouping,
    Sorting,
    /// Menus of the search query bar. They live here with the rest because at
    /// most one menu is open per view, and the query bar stands in the same view
    /// as the filter bar.
    Mission,
    Period,
    Cloud,
    /// A row menu, named by the row's key (see `Row::key`).
    Row(String),
}

impl Menu {
    /// Key in the markup message. A row menu is named by the row's key — that
    /// is also how it is addressed; a row has no second name.
    pub fn key(&self) -> String {
        match self {
            Menu::Filter => "filter".to_string(),
            Menu::Grouping => "grouping".to_string(),
            Menu::Sorting => "sorting".to_string(),
            Menu::Mission => "mission".to_string(),
            Menu::Period => "period".to_string(),
            Menu::Cloud => "cloud".to_string(),
            Menu::Row(row) => format!("row:{}", row),
        }
    }

    /// `None` — close what is open: both "close" and an unknown name arrive as
    /// an empty name, and there is nothing to show for it.
    pub fn from_key(key: &str) -> Option<Menu> {
        Some(match key {
            "filter" => Menu::Filter,
            "grouping" => Menu::Grouping,
            "sorting" => Menu::Sorting,
            "mission" => Menu::Mission,
            "period" => Menu::Period,
            "cloud" => Menu::Cloud,
            other => Menu::Row(other.strip_prefix("row:")?.to_string()),
        })
    }
}

/// The default is assembled from field defaults: each has its own, named where
/// the type itself lives — a second copy of this list would drift from them
/// silently.
#[derive(Default)]
pub struct ListingState {
    pub filter: Filter,
    pub grouping: Grouping,
    pub sorting: Sorting,
    /// Filter by name — what is typed in the filter field.
    pub query: String,
    /// The page, counting from zero. Any filter edit returns to the first one:
    /// "page 3" of a one-row list is a mistake, not a state.
    pub page: usize,
    /// Rows expanded into their content. A set of names, not a flag on the row:
    /// rows are rebuilt every frame from the library and the listing, while
    /// "I expanded this" is a property of the screen and survives the rebuild.
    pub expanded: std::collections::HashSet<String>,
    /// Selected rows: row key → what stands behind it. Each tab has its own and
    /// it survives a rebuild for the same reason as `expanded`.
    pub selected: std::collections::HashMap<String, Chosen>,
    /// The scroll-aim request number — grows on every jump to a row.
    ///
    /// "Aim" cannot be keyed by the row itself: the same row is targeted twice
    /// in a row, and the markup comes out identical the second time — the
    /// renderer sees the previous offset and decides it has already aimed (see
    /// `Scrollable.scroll_to` in ui-service/types.proto).
    pub aim: u64,
}

/// What stands behind a selected row.
///
/// A kind, not a "gets outlined" flag: a row is selected to do something with
/// it — delete, download — and that is a different action for a snapshot and
/// for a single file. The library does not know snapshots at all; they are
/// expanded into files (see `handlers::library::files_of`), while a file is
/// addressed by its own keys. The kind says nothing about the globe: outline
/// and shown are separate snapshot states (see handlers::outline).
///
/// The file carries its keys with it rather than taking them from the row at
/// action time: the selection survives a move to another folder, and the row
/// may already be off the screen by then.
#[derive(Clone, PartialEq, Eq)]
pub enum Chosen {
    /// A snapshot — one logical unit of the catalogue. Its selection key is the
    /// snapshot key itself, so it needs no fields of its own.
    Snapshot,
    /// A file on its own: downloaded outside a snapshot or unknown to one.
    File {
        /// Provider key; empty — nothing to download with.
        identifier: String,
        /// The snapshot the file belongs to: the provider knows the snapshot
        /// boundary, and the download asks for it along with the key.
        product: String,
        /// The record name in the library; empty — it is not on disk, nothing
        /// to delete.
        name: String,
    },
}

impl ListingState {
    /// Expand a row into its content or collapse it back. `true` — it is now
    /// expanded: folder content loads lazily and must be requested only on
    /// expansion (see handlers::browse::request_children).
    pub fn expand(&mut self, key: String) -> bool {
        if self.expanded.remove(&key) {
            return false;
        }
        self.expanded.insert(key);
        true
    }

    /// Select a row or clear the selection.
    pub fn select(&mut self, key: String, chosen: Chosen) {
        if self.selected.remove(&key).is_none() {
            self.selected.insert(key, chosen);
        }
    }
}
