use crate::module::components::RowStatus;

pub trait Choice: Copy + PartialEq + Sized + 'static {
    const ALL: &'static [Self];
    fn key(self) -> &'static str;
    fn title(self) -> &'static str;
    fn label(self) -> &'static str {
        self.title()
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|choice| choice.key() == key)
    }
}

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
    Grouping {
        None = "none", "No grouping", "none";
        Folder = "folder", "By folder", "folder";
        Tree = "tree", "Full tree", "tree";
    }
}

choice! {
    Sorting {
        Newest = "newest", "Newest first", "newest";
        Name   = "name",   "By name",      "name";
        Size   = "size",   "By size",      "size";
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Menu {
    Filter,
    Grouping,
    Sorting,
    Mission,
    Period,
    Cloud,
    Row(String),
}

impl Menu {
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

#[derive(Default)]
pub struct ListingState {
    pub filter: Filter,
    pub grouping: Grouping,
    pub sorting: Sorting,
    pub query: String,
    pub page: usize,
    pub expanded: std::collections::HashSet<String>,
    pub selected: std::collections::HashMap<String, Chosen>,
    pub aim: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Chosen {
    Snapshot,
    File {
        identifier: String,
        product: String,
        name: String,
    },
}

impl ListingState {
    pub fn expand(&mut self, key: String) -> bool {
        if self.expanded.remove(&key) {
            return false;
        }
        self.expanded.insert(key);
        true
    }

    pub fn select(&mut self, key: String, chosen: Chosen) {
        if self.selected.remove(&key).is_none() {
            self.selected.insert(key, chosen);
        }
    }
}
