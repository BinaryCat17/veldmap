use std::sync::{Arc, Mutex};
use veldmap_api::ui::UiEventResponse;
use veldsdk::core::Command;

use crate::state::State;

/// Поиск запрошен
pub fn on_search(
    state: Arc<Mutex<State>>,
    _request: UiEventResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    let query = guard.search.query.clone();
    
    if !query.is_empty() {
        guard.search.is_loading = true;
        veldsdk::publish!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
            query,
            filters: vec![],
        });
    }
    
    crate::view::render(&guard);
    Command::none()
}

pub fn on_search_input(
    state: Arc<Mutex<State>>,
    request: UiEventResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.search.query = request.value;
    crate::view::render(&guard);
    Command::none()
}

/// Результат поиска
pub fn on_search_result(
    state: Arc<Mutex<State>>,
    response: veldmap_api::dataprovider::SearchResponse,
) -> Command<()> {
    let mut guard = state.lock().unwrap();
    guard.search.is_loading = false;
    guard.search.results = response.products;
    crate::view::render(&guard);
    Command::none()
}
