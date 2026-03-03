//! browse/state.rs

use crate::common::BrowserItem;

#[derive(Default)]
pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowserItem>,

    pub token_stack: Vec<String>,
    pub current_page_token: String,
    pub next_token: Option<String>,

    pub is_loading: bool,
}
