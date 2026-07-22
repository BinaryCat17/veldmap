use crate::proto::ui::proto::UiEventResponse;
use crate::proto::dataprovider::SearchResponse;
use crate::module::state::State;

pub fn on_input_search_input(state: &mut State, event: UiEventResponse) {
    state.search.query = event.value;
}

pub fn on_input_search(state: &mut State, _event: UiEventResponse) {
    let query = state.search.query.clone();
    
    if !query.is_empty() {
        state.search.is_loading = true;
        state.search.error = None;
        crate::calls::data_provider::search(&crate::proto::dataprovider::SearchRequest {
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

    if !response.error.is_empty() {
        state.search.error = Some(response.error);
        state.search.results = Vec::new();
        return;
    }
    state.search.error = None;

    state.search.results = response.products;
    // Рендер происходит автоматически в on_frame
}
