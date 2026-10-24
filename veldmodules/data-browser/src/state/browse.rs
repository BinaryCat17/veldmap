//! Network catalogue: what is shown in a folder and what is loaded under
//! expanded rows.

use std::collections::{HashMap, HashSet};

pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    /// Content of the expanded folders of this view.
    pub children: Children,
    /// How to show this list — the filter, the order, the page.
    pub listing: super::listing::ListingState,
    /// Last list_path error from data-provider (empty if the request succeeded)
    pub error: Option<String>,
    /// Waiting for the data-provider/on_list_path reply. Only the last one
    /// counts: two quick folder switches give two replies, and the first one's
    /// content under the second one's header is a lie, not stale data.
    pub request: veldsdk::Latest,
}

pub struct BrowseItem {
    /// The product path together with the bucket name (`eodata/Sentinel-2/…`) —
    /// in this form all data-provider topics accept it. The object key in the
    /// bucket lives under this prefix, but the provider cuts it off, not us.
    pub identifier: String,
    pub name: String,
    pub is_folder: bool,
    /// The snapshot the record lies in; empty — it is on its way to the
    /// snapshots. If it matches `identifier` without the trailing slash — the
    /// record is the snapshot itself. The provider said it: deriving the snapshot
    /// boundary from the key has nothing to go on here (see `ListEntry.product`).
    pub product: String,
    /// Object size in bytes; 0 — a folder or unknown size.
    pub size: u64,
    /// Object time, unix seconds; 0 — unknown.
    pub modified: i64,
    /// Why not to offer "to the globe"; empty — offer it. The provider said it,
    /// because only it has the list of readable formats (see
    /// `ListEntry.unviewable`).
    pub unviewable: String,
}

impl BrowseState {
    /// Whether the tab shows this folder right now — that is, whether the
    /// network hop can be skipped. A freshly opened one shows nothing, even when
    /// the path matches: an empty tab has an empty path too.
    pub fn shows(&self, path: &str) -> bool {
        self.current_path == path && (!self.items.is_empty() || self.request.is_pending())
    }
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            children: Children::default(),
            listing: Default::default(),
            error: None,
            request: veldsdk::Latest::new(),
        }
    }
}

/// Content of expanded folders: folder key → its entries.
///
/// One device for both the catalogue and the search: an expanded row shows the
/// same provider listing in both, and a second way to remember it would diverge
/// from the first. Paged lazily — a folder is asked when it is expanded, not when
/// its row is shown: a bucket root has hundreds of folders, and traversing each
/// would cost hundreds of requests for the sake of one expanded one.
///
/// "Were we asking" and "have we got an answer" are different questions, and
/// they have different carriers. The first is answered by the presence of the
/// key: by it one can see whether to go to the network. The second by `waiting`,
/// and only one view needs it: an expanded empty folder and an expanded one whose
/// answer is still in flight look the same, and the latter reads as "nothing
/// here" for a second.
#[derive(Default)]
pub struct Children {
    /// The key is present — the folder was asked; no key — it was not asked.
    loaded: HashMap<String, Vec<BrowseItem>>,
    /// Folders whose listing is still in flight.
    waiting: HashSet<String>,
}

impl Children {
    /// Entries of an expanded folder; empty — they have not arrived yet or there
    /// is nothing to fill it with.
    pub fn get(&self, key: &str) -> &[BrowseItem] {
        self.loaded.get(key).map_or(&[], Vec::as_slice)
    }

    /// Whether this folder has already been asked. `false` — it must be asked.
    pub fn known(&self, key: &str) -> bool {
        self.loaded.contains_key(key)
    }

    /// Whether we are still waiting for an answer for this folder — what the
    /// waiting row shows.
    pub fn waiting(&self, key: &str) -> bool {
        self.waiting.contains(key)
    }

    /// The folder listing has started.
    pub fn begin(&mut self, key: String) {
        self.loaded.insert(key.clone(), Vec::new());
        self.waiting.insert(key);
    }

    /// The answer arrived in full — or did not arrive at all. There is nothing
    /// left to wait for.
    pub fn settle(&mut self, key: &str) {
        self.waiting.remove(key);
    }

    /// The next page of the listing. `false` — the folder was forgotten meanwhile:
    /// we left it or the results changed.
    ///
    /// Exactly `false`, and not a silent insert: pages come one by one, and an
    /// abandoned chain continues from the token where it was left. Were it to
    /// re-create the key, the set would end up with a listing tail without a head,
    /// and the expanded folder would show content starting from the middle.
    pub fn extend(&mut self, key: &str, items: impl IntoIterator<Item = BrowseItem>) -> bool {
        let Some(into) = self.loaded.get_mut(key) else { return false };
        into.extend(items);
        true
    }

    /// Forget one folder. That is how one whose listing refused goes: "were
    /// asking" is not the same as "asked", and a left-behind key would mean "empty
    /// forever".
    pub fn forget(&mut self, key: &str) {
        self.loaded.remove(key);
        self.waiting.remove(key);
    }

    /// Forget everything — that is how the content belonging to the previous
    /// folder or the previous results goes.
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.waiting.clear();
    }
}
