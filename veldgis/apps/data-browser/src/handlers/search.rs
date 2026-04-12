use std::sync::{Arc, Mutex};
use veld_ui::proto::UiEventResponse;
use veldmap_api::dataprovider::SearchResponse;
use crate::state::State;

pub fn on_search_input(state: Arc<Mutex<State>>, event: UiEventResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.search.query = event.value;
    Ok(())
}

pub fn on_search(state: Arc<Mutex<State>>, _event: UiEventResponse) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    let query = guard.search.query.clone();
    
    if !query.is_empty() {
        guard.search.is_loading = true;
        veldsdk::publish!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
            query,
            filters: vec![],
        });
    }
    Ok(())
}

/// Результат поиска
pub fn on_search_result(
    state: Arc<Mutex<State>>,
    response: SearchResponse,
) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.search.is_loading = false;
    guard.search.results = response.products;
    
    crate::view::render(&mut guard);
    Ok(())
}
