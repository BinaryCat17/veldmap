use std::sync::{Arc, Mutex};
use crate::state::State;

/// Поиск запрошен
pub fn on_search(
    state: &mut State,
    _value: String,
) -> anyhow::Result<()> {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        veldsdk::publish!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
            query,
            filters: vec![],
        });
    }
    
    Ok(())
}

pub fn on_search_input(
    state: &mut State,
    value: String,
) -> anyhow::Result<()> {
    state.search.query = value;
    Ok(())
}

/// Результат поиска
pub fn on_search_result(
    state: Arc<Mutex<State>>,
    response: veldmap_api::dataprovider::SearchResponse,
) -> anyhow::Result<()> {
    let mut guard = state.lock().unwrap();
    guard.search.is_loading = false;
    guard.search.results = response.products;
    crate::view::render(&mut guard);
    Ok(())
}
