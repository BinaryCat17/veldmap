use veld_ui::proto::UiEventResponse;
use veldmap_api::dataprovider::SearchResponse;
use crate::state::State;

pub fn on_search_input(state: &mut State, event: UiEventResponse) {
    state.search.query = event.value;
}

pub fn on_search(state: &mut State, _event: UiEventResponse) {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        veldsdk::call!("data-provider/search", veldmap_api::dataprovider::SearchRequest {
            query,
            filters: vec![],
        });
    }
}

/// Результат поиска
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    state.search.is_loading = false;
    state.search.results = response.products;
    // Рендер происходит автоматически в on_frame
}
