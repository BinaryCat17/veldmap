use crate::proto::ui_service::proto::UiEventResponse;
use crate::proto::data_provider::SearchResponse;
use crate::module::state::State;

pub fn on_input_search_input(state: &mut State, event: UiEventResponse) {
    state.search.query = event.value;
}

pub fn on_input_search(state: &mut State, _event: UiEventResponse) {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        veldsdk::call!("data-provider/search", crate::proto::data_provider::SearchRequest {
            query,
            filters: vec![],
        });
    }
}

/// Результат поиска
pub fn on_sub_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    state.search.is_loading = false;
    state.search.results = response.products;
    // Рендер происходит автоматически в on_frame
}
