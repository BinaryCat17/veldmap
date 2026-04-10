//! browse/state.rs

use crate::common::BrowserItem;

#[derive(Clone)]
pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowserItem>,

    pub page_tokens: Vec<String>,
    pub current_page: usize,

    pub is_loading: bool,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            page_tokens: vec![String::new()],
            current_page: 0,
            is_loading: false,
        }
    }
}
