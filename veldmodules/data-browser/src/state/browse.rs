use std::collections::{HashMap, HashSet};

pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    pub children: Children,
    pub listing: super::listing::ListingState,
    pub error: Option<String>,
    pub request: veldsdk::Latest,
}

pub struct BrowseItem {
    pub identifier: String,
    pub name: String,
    pub is_folder: bool,
    pub product: String,
    pub size: u64,
    pub modified: i64,
    pub unviewable: String,
}

impl BrowseState {
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

#[derive(Default)]
pub struct Children {
    loaded: HashMap<String, Vec<BrowseItem>>,
    waiting: HashSet<String>,
}

impl Children {
    pub fn get(&self, key: &str) -> &[BrowseItem] {
        self.loaded.get(key).map_or(&[], Vec::as_slice)
    }

    pub fn known(&self, key: &str) -> bool {
        self.loaded.contains_key(key)
    }

    pub fn waiting(&self, key: &str) -> bool {
        self.waiting.contains(key)
    }

    pub fn begin(&mut self, key: String) {
        self.loaded.insert(key.clone(), Vec::new());
        self.waiting.insert(key);
    }

    pub fn settle(&mut self, key: &str) {
        self.waiting.remove(key);
    }

    pub fn extend(&mut self, key: &str, items: impl IntoIterator<Item = BrowseItem>) -> bool {
        let Some(into) = self.loaded.get_mut(key) else { return false };
        into.extend(items);
        true
    }

    pub fn forget(&mut self, key: &str) {
        self.loaded.remove(key);
        self.waiting.remove(key);
    }

    pub fn clear(&mut self) {
        self.loaded.clear();
        self.waiting.clear();
    }
}
